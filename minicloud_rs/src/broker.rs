//! Small MQTT 3.1.1 subset. mqttbytes owns packet framing/encoding; this
//! module owns bounded routing. No Tokio runtime or thread per MQTT client.
use crate::{Error, Result, Shared, MAX_PAYLOAD};
use bytes::{Buf, BytesMut};
use mqttbytes::{
    v4::{self, *},
    QoS,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};

const MAX_CLIENTS: usize = 4;
const PACKET_LIMIT: usize = 3072;
const OUTPUT_LIMIT: usize = 4096;
#[derive(Default)]
pub struct Hub {
    clients: BTreeMap<u64, Subscriber>,
    retained: BTreeMap<String, Vec<u8>>,
}
#[derive(Default)]
struct Subscriber {
    filters: Vec<String>,
    output: BytesMut,
    overflow: bool,
}
impl Hub {
    fn add(&mut self, id: u64) {
        self.clients.insert(id, Subscriber::default());
    }
    fn remove(&mut self, id: u64) {
        self.clients.remove(&id);
    }
    fn queue(&mut self, id: u64, bytes: &[u8]) {
        if let Some(client) = self.clients.get_mut(&id) {
            if client.output.len() + bytes.len() > OUTPUT_LIMIT {
                client.overflow = true;
            } else {
                client.output.extend_from_slice(bytes);
            }
        }
    }
    pub fn publish(&mut self, topic: &str, payload: &[u8], retain: bool) -> Result<()> {
        if topic.len() > 128 || !mqttbytes::valid_topic(topic) || payload.len() > MAX_PAYLOAD {
            return Err(Error::new(400, "invalid topic or oversized MQTT payload"));
        }
        if retain {
            if payload.is_empty() {
                self.retained.remove(topic);
            } else {
                if !self.retained.contains_key(topic) && self.retained.len() == 8 {
                    return Err(Error::new(429, "retained topic limit reached"));
                }
                self.retained.insert(topic.into(), payload.into());
            }
        }
        let mut packet = BytesMut::new();
        Publish::new(topic, QoS::AtMostOnce, payload)
            .write(&mut packet)
            .map_err(protocol)?;
        let ids: Vec<_> = self
            .clients
            .iter()
            .filter(|(_, s)| s.filters.iter().any(|f| mqttbytes::matches(topic, f)))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.queue(id, &packet);
        }
        Ok(())
    }
    fn subscribe(&mut self, id: u64, subscribe: Subscribe) -> Result<()> {
        let client = self
            .clients
            .get_mut(&id)
            .ok_or_else(|| Error::new(400, "missing client"))?;
        if subscribe.filters.is_empty() || subscribe.pkid == 0 {
            return Err(Error::new(400, "invalid subscribe"));
        }
        let mut codes = Vec::new();
        let mut added = Vec::new();
        for filter in subscribe.filters {
            if filter.path.len() > 128
                || !mqttbytes::valid_filter(&filter.path)
                || filter.path.starts_with("$share/")
                || client.filters.len() >= 8
            {
                codes.push(SubscribeReasonCode::Failure);
            } else {
                codes.push(SubscribeReasonCode::Success(QoS::AtMostOnce));
                if !client.filters.contains(&filter.path) {
                    client.filters.push(filter.path.clone());
                }
                added.push(filter.path);
            }
        }
        let mut ack = BytesMut::new();
        SubAck::new(subscribe.pkid, codes)
            .write(&mut ack)
            .map_err(protocol)?;
        self.queue(id, &ack);
        let mut retained = BytesMut::new();
        for (topic, payload) in &self.retained {
            if added.iter().any(|f| mqttbytes::matches(topic, f)) {
                let mut p = Publish::new(topic, QoS::AtMostOnce, payload.as_slice());
                p.retain = true;
                p.write(&mut retained).map_err(protocol)?;
            }
        }
        self.queue(id, &retained);
        Ok(())
    }
}
fn protocol(e: mqttbytes::Error) -> Error {
    Error::new(400, format!("MQTT: {e:?}"))
}
struct Client {
    id: u64,
    stream: TcpStream,
    input: BytesMut,
    authenticated: bool,
    name: String,
    seen: Instant,
    idle: Duration,
    closing: bool,
}
pub struct Broker {
    listener: TcpListener,
    clients: Vec<Client>,
    sequence: u64,
}
impl Broker {
    pub fn bind(address: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            clients: Vec::new(),
            sequence: 0,
        })
    }
    pub fn step(&mut self, shared: &Shared) {
        if let Ok((stream, _)) = self.listener.accept() {
            if self.clients.len() < MAX_CLIENTS && stream.set_nonblocking(true).is_ok() {
                let _ = stream.set_nodelay(true);
                self.sequence += 1;
                shared.lock().unwrap().broker.add(self.sequence);
                self.clients.push(Client {
                    id: self.sequence,
                    stream,
                    input: BytesMut::with_capacity(PACKET_LIMIT),
                    authenticated: false,
                    name: String::new(),
                    seen: Instant::now(),
                    idle: Duration::from_secs(10),
                    closing: false,
                });
            }
        }
        let mut removed = Vec::new();
        for client in &mut self.clients {
            if client.seen.elapsed() > client.idle {
                removed.push(client.id);
                continue;
            }
            if !client.closing {
                let mut chunk = [0; 1024];
                match client.stream.read(&mut chunk) {
                    Ok(0) => {
                        removed.push(client.id);
                        continue;
                    }
                    Ok(n) => {
                        if client.input.len() + n > PACKET_LIMIT {
                            removed.push(client.id);
                            continue;
                        }
                        client.input.extend_from_slice(&chunk[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => {
                        removed.push(client.id);
                        continue;
                    }
                }
                for _ in 0..8 {
                    match v4::read(&mut client.input, PACKET_LIMIT) {
                        Ok(packet) => {
                            client.seen = Instant::now();
                            if Self::packet(client, packet, shared).is_err() {
                                removed.push(client.id);
                                break;
                            }
                        }
                        Err(mqttbytes::Error::InsufficientBytes(_)) => break,
                        Err(_) => {
                            removed.push(client.id);
                            break;
                        }
                    }
                }
            }
            let mut cloud = shared.lock().unwrap();
            if let Some(subscriber) = cloud.broker.clients.get_mut(&client.id) {
                if subscriber.overflow {
                    removed.push(client.id);
                    continue;
                }
                if !subscriber.output.is_empty() {
                    match client.stream.write(&subscriber.output) {
                        Ok(0) => removed.push(client.id),
                        Ok(n) => subscriber.output.advance(n),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(_) => removed.push(client.id),
                    }
                }
                if client.closing && subscriber.output.is_empty() {
                    removed.push(client.id);
                }
            }
        }
        // MQTT requires an existing connection with the same client ID to close.
        for i in 0..self.clients.len() {
            for j in i + 1..self.clients.len() {
                if self.clients[i].authenticated
                    && self.clients[j].authenticated
                    && self.clients[i].name == self.clients[j].name
                {
                    removed.push(self.clients[i].id);
                }
            }
        }
        self.clients.retain(|c| !removed.contains(&c.id));
        let mut cloud = shared.lock().unwrap();
        for id in removed {
            cloud.broker.remove(id);
        }
    }
    fn packet(client: &mut Client, packet: Packet, shared: &Shared) -> Result<()> {
        let mut cloud = shared.lock().unwrap();
        let mut response = BytesMut::new();
        if let Packet::Connect(connect) = packet {
            if client.authenticated {
                return Err(Error::new(400, "duplicate CONNECT"));
            }
            let code = if connect.protocol != mqttbytes::Protocol::V4
                || !connect.clean_session
                || connect.last_will.is_some()
            {
                ConnectReturnCode::RefusedProtocolVersion
            } else if !crate::name(&connect.client_id) {
                ConnectReturnCode::BadClientId
            } else if !connect
                .login
                .as_ref()
                .is_some_and(|l| l.password == cloud.token)
            {
                ConnectReturnCode::NotAuthorized
            } else {
                ConnectReturnCode::Success
            };
            ConnAck::new(code, false)
                .write(&mut response)
                .map_err(protocol)?;
            client.authenticated = code == ConnectReturnCode::Success;
            client.closing = !client.authenticated;
            client.name = connect.client_id;
            client.idle = if connect.keep_alive == 0 {
                Duration::from_secs(3600)
            } else {
                Duration::from_millis(connect.keep_alive as u64 * 1500)
            };
        } else {
            if !client.authenticated {
                return Err(Error::new(401, "CONNECT required"));
            }
            match packet {
                Packet::Publish(p) => {
                    if p.qos == QoS::ExactlyOnce || (p.qos == QoS::AtLeastOnce && p.pkid == 0) {
                        return Err(Error::new(400, "QoS2/invalid packet ID"));
                    }
                    if p.topic.len() > 128
                        || !mqttbytes::valid_topic(&p.topic)
                        || p.payload.len() > MAX_PAYLOAD
                        || matches!(
                            p.topic.as_str(),
                            "minicloud/jobs/result" | "minicloud/screen/changed"
                        )
                    {
                        return Err(Error::new(
                            400,
                            "invalid/reserved topic or oversized payload",
                        ));
                    }
                    if cloud.plugins.iter().any(|plugin| plugin.accepts(&p.topic)) {
                        if p.retain {
                            return Err(Error::new(400, "commands must not be retained"));
                        }
                        let payload = serde_json::from_slice(&p.payload)?;
                        cloud.enqueue(&p.topic, payload, None)?;
                    }
                    cloud.broker.publish(&p.topic, &p.payload, p.retain)?;
                    if p.qos == QoS::AtLeastOnce {
                        PubAck::new(p.pkid).write(&mut response).map_err(protocol)?;
                    }
                }
                Packet::Subscribe(s) => cloud.broker.subscribe(client.id, s)?,
                Packet::Unsubscribe(u) => {
                    let s = cloud.broker.clients.get_mut(&client.id).unwrap();
                    s.filters.retain(|f| !u.topics.contains(f));
                    UnsubAck::new(u.pkid)
                        .write(&mut response)
                        .map_err(protocol)?;
                }
                Packet::PingReq => {
                    PingResp.write(&mut response).map_err(protocol)?;
                }
                Packet::Disconnect => client.closing = true,
                _ => return Err(Error::new(400, "unexpected packet")),
            }
        }
        cloud.broker.queue(client.id, &response);
        Ok(())
    }
}
