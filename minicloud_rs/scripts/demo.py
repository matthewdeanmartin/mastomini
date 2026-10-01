"""Send fictional Mastomini/NanaCoin notifications to your local prototype."""
import json
import os
from urllib.request import Request,urlopen

base=os.environ.get("MINICLOUD_URL","http://127.0.0.1:8090")
token=os.environ.get("MINICLOUD_ADMIN_TOKEN","local-prototype-token")
for source,recipient,text,size in [
    ("mastomini","Alex","You have a new household message.","medium"),
    ("nanacoin","Sam","You have new NanaCoin mail.","large"),
]:
    event={"event_id":f"demo-{source}-1","source":source,"id":"demo-1","recipient":recipient,"text":text,"size":size}
    request=Request(base+"/api/screen/notify",data=json.dumps(event).encode(),headers={"Authorization":"Bearer "+token,"Content-Type":"application/json"},method="POST")
    with urlopen(request) as response:
        print(response.read().decode())
print(base+"/ — public screen; anyone can dismiss these notifications")
