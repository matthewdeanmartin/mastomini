import { Component, signal, OnDestroy } from '@angular/core';
import { bootstrapApplication } from '@angular/platform-browser';
import { FormsModule } from '@angular/forms';

interface BlobItem { bucket: string; key: string; mime: string; size: number; hash: string }
interface Notice { source: string; id: string; recipient: string; text: string; size: string; image?: {bucket: string; key: string} }
interface Receipt { id: string; status: string; detail: unknown }
@Component({
  selector: 'app-root', imports: [FormsModule],
  template: `
    <header><a class="brand" href="/">mini<span>cloud</span><small>HOUSEHOLD INFRASTRUCTURE</small></a>
      <nav><a href="/" [class.active]="!admin">Kitchen screen</a><a href="/admin" [class.active]="admin">Management</a></nav>
      <span class="connection" [class.offline]="!online()">{{ online() ? '● Connected' : '○ Reconnecting' }}</span>
    </header>
    <main>
      @if (error()) { <div class="error" role="alert">{{ error() }} <button (click)="error.set('')">Close</button></div> }
      @if (success()) { <div class="success" role="status">{{ success() }}</div> }
      <div class="heading"><div><p class="eyebrow">{{ admin ? 'YOUR SMALL CLOUD' : 'AROUND THE HOUSE' }}</p>
        <h1>{{ admin ? 'A little infrastructure.' : 'Something for you.' }}</h1>
        <p>{{ admin ? 'Files, messages and small pieces of work, all in one place.' : 'Household updates from NanaCoin, Mastomini and your apps.' }}</p></div>
        <span class="pill">ESP32-C6 · 320 × 172</span></div>
      @if (admin && !unlocked()) {
        <section class="panel login"><h2>Open management</h2><p>Use your minicloud admin token. The kitchen screen is public.</p>
          <label>Admin token<input type="password" [(ngModel)]="token" autocomplete="off" (keydown.enter)="unlock()"></label>
          <button class="primary" (click)="unlock()">Connect</button><small>Local prototype default: local-prototype-token</small>
        </section>
      } @else {
        @if (admin) {
          <div class="tabs">@for (name of ['Files', 'Screen', 'Workers']; track name) { <button [class.selected]="tab() === name" (click)="tab.set(name)">{{ name }}</button> }
            <button class="logout" (click)="lock()">Lock management</button></div>
        }
        @if (!admin || tab() === 'Screen') {
          <div class="screen-layout">
            <section class="preview-panel"><div class="eyebrow">KITCHEN DISPLAY</div>
              <div class="device"><div class="lcd" [class.small]="current()?.size === 'small'" [class.large]="current()?.size === 'large'">
                @if (current(); as notice) {
                  <div class="lcd-source">{{ notice.source }}</div><div class="lcd-person">{{ notice.recipient }}</div>
                  @if (screenImage()) { <img [src]="screenImage()" alt="Notification image"> }
                  <div class="lcd-text">{{ pageText() }}</div><div class="lcd-footer">Page {{ page() }} / {{ pages() }} - {{ notices().length }} pending · rotates every 8s</div>
                } @else { <div class="idle"><span>☁</span><h2>All caught up.</h2><p>Enjoy the quiet.</p></div> }
              </div></div><button (click)="next()">Next message →</button><p class="caption">Landscape preview, scaled to fit. No button press needed.</p>
            </section>
            <section><h2>Waiting for someone <span class="count">{{ notices().length }}</span></h2>
              @for (notice of notices(); track notice.source + '/' + notice.id) {
                <article class="notice"><div><span class="tag">{{ notice.source }}</span><h3>{{ notice.recipient || 'Household' }}</h3><p>{{ notice.text }}</p></div>
                  <button (click)="dismiss(notice)">Dismiss ✓</button></article>
              } @empty { <div class="empty">No unread updates. New messages will appear here automatically.</div> }
              @if (admin) {
                <section class="panel composer"><h2>Send to the screen</h2>
                  <div class="two"><label>App / source<input [(ngModel)]="source"></label><label>For<input [(ngModel)]="recipient"></label></div>
                  <label>Text<textarea rows="3" [(ngModel)]="message" maxlength="256"></textarea></label>
                  <div class="two"><label>Text size<select [(ngModel)]="size"><option>small</option><option>medium</option><option>large</option></select></label>
                    <label>Image<select [(ngModel)]="image"><option value="">No image</option>@for (blob of imageBlobs(); track blob.hash + blob.key) { <option [value]="blob.bucket + '/' + blob.key">{{ blob.key }}</option> }</select></label></div>
                  <button class="primary" (click)="notify()">Send notification</button>
                </section>
              }
            </section>
          </div>
        }
        @if (admin && tab() === 'Files') {
          <div class="stats"><section><span>STORED FILES</span><strong>{{ blobs().length }} <small>/ 48</small></strong></section>
            <section><span>FLASH BUDGET</span><strong>{{ storageKB() }} <small>KiB / 3 MiB</small></strong></section>
            <section><span>FILE URLS</span><strong class="small-stat">Ready to serve</strong><small>GET · MIME type · streamed</small></section></div>
          <section class="panel upload"><div><h2>Put something in the cloud</h2><p>Images and files up to 256 KiB each. Buckets keep apps organized.</p></div>
            <label>Bucket<input [(ngModel)]="bucket"></label><label>File<input type="file" (change)="choose($event)"></label>
            <label class="checkbox"><input type="checkbox" [(ngModel)]="prepareLCD"> Also prepare image for the LCD</label>
            <button class="primary" [disabled]="!file || busy()" (click)="upload()">{{ busy() ? 'Uploading…' : 'Upload' }}</button></section>
          <div class="file-toolbar"><h2>File browser</h2><label>Bucket<select [(ngModel)]="filter"><option value="">All buckets</option>@for (b of buckets(); track b) { <option>{{ b }}</option> }</select></label></div>
          <div class="files">@for (blob of filteredBlobs(); track blob.bucket + '/' + blob.key) {
            <article class="file"><div class="file-preview">@if (blob.mime.startsWith('image/')) { <img [src]="url(blob)" [alt]="blob.key" loading="lazy"> } @else { <span>{{ blob.mime === 'application/x-rgb565' ? 'LCD' : 'FILE' }}</span> }</div>
              <div class="file-info"><span class="tag">{{ blob.bucket }}</span><h3>{{ blob.key }}</h3><p>{{ blob.mime }} · {{ (blob.size / 1024).toFixed(1) }} KiB</p>
                <div class="actions"><a [href]="url(blob)" target="_blank" rel="noopener">Open file ↗</a><button (click)="copyURL(blob)">Copy URL</button><button class="danger" (click)="remove(blob)">Delete</button></div></div></article>
          } @empty { <div class="empty">Your files will live here. Upload an image to get started.</div> }</div>
        }
        @if (admin && tab() === 'Workers') {
          <div class="worker-grid"><section class="panel"><p class="eyebrow">COMPILED PLUGINS</p><h2>Small programs. Useful work.</h2>
            @for (plugin of plugins(); track plugin) { <div class="plugin"><strong>{{ plugin }}</strong><span class="pill">Rust · compiled in</span></div> }
            <p>MQTT commands and HTTP invocations enter the same durable queue. Workers retry storage failures and keep the latest 32 results.</p>
            <label>Plugin<select [(ngModel)]="pluginName">@for (plugin of plugins(); track plugin) { <option>{{ plugin }}</option> }</select></label>
            <label>Topic<input [(ngModel)]="topic"></label><label>JSON payload<textarea class="code" rows="7" [(ngModel)]="payload"></textarea></label>
            <button class="primary" (click)="invoke()">Invoke plugin</button></section>
            <section><h2>Work queue <span class="count">{{ queued().length }}</span></h2>
              @for (job of queued(); track job.id) { <article class="result"><span class="tag">QUEUED</span><strong>{{ job.id }}</strong><p>Attempt {{ job.attempts + 1 }}</p></article> }
              <h2>Recent results</h2>@for (r of receipts(); track r.id) { <article class="result"><span class="tag" [class.failed]="r.status === 'failed'">{{ r.status }}</span><strong>{{ r.id }}</strong><pre>{{ stringify(r.detail) }}</pre></article> }
              @if (!receipts().length) { <div class="empty">No invocations yet.</div> }
            </section></div>
        }
      }
      <footer>MINICLOUD <span>A household cloud, on a household scale.</span></footer>
    </main>
  `
})
class App implements OnDestroy {
  admin = location.pathname === '/admin';
  token = ''; unlocked = signal(false); online = signal(false); busy = signal(false);
  error = signal(''); success = signal(''); tab = signal('Files');
  blobs = signal<BlobItem[]>([]); notices = signal<Notice[]>([]); current = signal<Notice | null>(null); screenImage = signal(''); pageText=signal(''); page=signal(1); pages=signal(1);
  plugins = signal<string[]>([]); receipts = signal<Receipt[]>([]); queued = signal<{id: string; attempts: number}[]>([]);
  bucket = 'household'; filter = ''; file: File | null = null; prepareLCD = true;
  source = 'mastomini'; recipient = 'Household'; message = 'You have a new message.'; size = 'medium'; image = '';
  pluginName = 'screen'; topic = 'minicloud/screen/notify';
  payload = JSON.stringify({event_id: 'demo-1', source: 'nanacoin', id: 'demo-1', recipient: 'Household', text: 'You have new NanaCoin mail.', size: 'large'}, null, 2);
  timer: ReturnType<typeof setInterval>; imageIdentity = ''; imageGeneration = 0;
  constructor() { void this.refresh(); this.timer = setInterval(() => void this.refresh(), 2500); }
  ngOnDestroy() { clearInterval(this.timer); }
  headers() { return {Authorization: 'Bearer ' + this.token}; }
  async api(path: string, options: RequestInit = {}): Promise<any> {
    const response = await fetch(path, { ...options, headers: { ...this.headers(), ...options.headers } });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
    return result;
  }
  async refresh() {
    try {
      // Sequential requests respect the board's single HTTP connection budget.
      const status = await this.api('/api/status'); this.plugins.set(status.plugins); this.online.set(true);
      const screen = await this.api('/api/screen'); this.notices.set(screen.notices); this.current.set(screen.current); this.pageText.set(screen.page_text ?? screen.current?.text ?? ''); this.page.set(screen.page ?? 1); this.pages.set(screen.pages ?? 1);
      await this.showImage(screen.current, screen.revision);
      if (this.admin && this.unlocked()) {
        this.blobs.set((await this.api('/api/blobs')).blobs);
        const jobs = await this.api('/api/jobs'); this.queued.set(jobs.queued); this.receipts.set(jobs.receipts.slice().reverse());
      }
    } catch { this.online.set(false); }
  }
  async action(fn: () => Promise<unknown>, message: string) {
    this.error.set(''); this.success.set('');
    try { await fn(); this.success.set(message); await this.refresh(); }
    catch (error) { this.error.set(String(error instanceof Error ? error.message : error)); }
  }
  async unlock() { await this.action(async () => { await this.api('/api/blobs'); this.unlocked.set(true); }, 'Management connected.'); }
  lock() { this.unlocked.set(false); this.token = ''; this.blobs.set([]); this.receipts.set([]); }
  url(blob: {bucket: string; key: string}) { return '/blobs/' + encodeURIComponent(blob.bucket) + '/' + blob.key.split('/').map(encodeURIComponent).join('/'); }
  stringify(value: unknown) { return JSON.stringify(value, null, 2); }
  buckets() { return [...new Set(this.blobs().map(b => b.bucket))].sort(); }
  filteredBlobs() { return this.blobs().filter(b => !this.filter || b.bucket === this.filter); }
  imageBlobs() { return this.blobs().filter(b => b.mime.startsWith('image/') || b.mime === 'application/x-rgb565'); }
  storageKB() { return (this.blobs().reduce((n,b) => n + b.size, 0) / 1024).toFixed(0); }
  choose(event: Event) { this.file = (event.target as HTMLInputElement).files?.[0] || null; }
  async put(key: string, mime: string, bytes: Blob | Uint8Array<ArrayBuffer>) {
    const response = await fetch('/api/blobs/' + encodeURIComponent(this.bucket) + '/' + encodeURIComponent(key), {method: 'PUT', headers: {...this.headers(), 'Content-Type': mime}, body: bytes});
    if (!response.ok) throw new Error((await response.json()).error);
  }
  async upload() {
    if (!this.file) return;
    const file = this.file; this.busy.set(true);
    await this.action(async () => {
      await this.put(file.name, file.type || 'application/octet-stream', file);
      if (this.prepareLCD && file.type.startsWith('image/')) {
        const bitmap = await createImageBitmap(file); const canvas = document.createElement('canvas'); canvas.width = 320; canvas.height = 172;
        const context = canvas.getContext('2d')!; context.fillStyle = '#07181c'; context.fillRect(0,0,320,172);
        const ratio = Math.min(320 / bitmap.width, 172 / bitmap.height); const w = bitmap.width * ratio, h = bitmap.height * ratio;
        context.drawImage(bitmap, (320-w)/2, (172-h)/2, w, h); bitmap.close();
        const pixels = context.getImageData(0,0,320,172).data; const bytes = new Uint8Array(172*320*2);
        for (let p=0; p<172*320; p++) { const rgb = ((pixels[p*4] >> 3) << 11) | ((pixels[p*4+1] >> 2) << 5) | (pixels[p*4+2] >> 3); bytes[p*2] = rgb >> 8; bytes[p*2+1] = rgb & 255; }
        await this.put(file.name + '.rgb565', 'application/x-rgb565', bytes);
      }
    }, 'File uploaded. Open its URL or send it to the screen.');
    this.busy.set(false);
  }
  remove(blob: BlobItem) { void this.action(() => this.api('/api/blobs/' + blob.bucket + '/' + blob.key, {method: 'DELETE'}), 'File deleted.'); }
  copyURL(blob: BlobItem) { void this.action(() => navigator.clipboard.writeText(new URL(this.url(blob), location.origin).href), 'File URL copied.'); }
  post(path: string, payload: unknown) { return this.api(path, {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(payload)}); }
  notify() {
    const id = 'web-' + Date.now(); const i = this.image.indexOf('/');
    const payload = {event_id: id, source: this.source, id, recipient: this.recipient, text: this.message, size: this.size, image: this.image ? {bucket: this.image.slice(0,i), key: this.image.slice(i+1)} : null};
    void this.action(() => this.post('/api/screen/notify', payload), 'Notification queued. Check Workers for its result.');
  }
  dismiss(n: Notice) { void this.action(() => this.post('/api/screen/' + n.source + '/' + n.id + '/dismiss', {}), 'Notification dismissed.'); }
  next() { void this.action(() => this.post('/api/screen/next', {}), 'Showing the next message.'); }
  invoke() { void this.action(async () => this.post('/api/plugins/' + this.pluginName + '/invoke', {topic: this.topic, payload: JSON.parse(this.payload)}), 'Invocation queued.'); }
  async showImage(n: Notice | null, revision: number) {
    const key = n?.image ? this.url(n.image) : ''; const identity = key + ':' + revision; if (identity === this.imageIdentity) return;
    this.imageIdentity = identity; const generation = ++this.imageGeneration; this.screenImage.set(''); if (!key) return;
    const response = await fetch(key); if (!response.ok) return;
    if (response.headers.get('Content-Type') !== 'application/x-rgb565') { if (generation === this.imageGeneration) this.screenImage.set(key); return; }
    const bytes = new Uint8Array(await response.arrayBuffer()); if (bytes.length !== 110080) return;
    const canvas = document.createElement('canvas'); canvas.width=320; canvas.height=172;
    const context = canvas.getContext('2d')!; const image = context.createImageData(320,172);
    for (let p=0;p<172*320;p++) { const rgb=(bytes[p*2]<<8)|bytes[p*2+1]; image.data[p*4]=((rgb>>11)&31)*255/31; image.data[p*4+1]=((rgb>>5)&63)*255/63; image.data[p*4+2]=(rgb&31)*255/31; image.data[p*4+3]=255; }
    context.putImageData(image,0,0); if (generation === this.imageGeneration) this.screenImage.set(canvas.toDataURL());
  }
}
bootstrapApplication(App).catch(console.error);
