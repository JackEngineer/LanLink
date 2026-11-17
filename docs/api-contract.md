# Local API contract
All routes use `/api/v1`. JSON response arrays are bare arrays. Errors use `{ "error": "human readable reason" }`. UI and API share the exact loopback origin. Mutations require `X-LANLink-Token` from `GET /api/v1/session` and exact Origin. GET session requires same-origin fetch header. WS supplies token using `Sec-WebSocket-Protocol` values `lanlink` and token. WS sends `{ "type": "changed" }` to invalidate caches and clients refetch after reconnect.

- GET health -> `{status:"ok",version:"0.1.0"}`
- GET session -> `{token:string}`
- GET device -> Device
- PATCH device `{name}` -> Device
- GET devices -> Peer[]
- POST devices/{id}/pair -> `{id:string}` pairing attempt
- DELETE devices/{id}/pair -> `{ok:true}` revokes
- GET pairings -> Pairing[]
- POST pairings/{id}/confirm `{accept:boolean}` -> `{ok:true}`
- GET messages?peer_id= -> Message[]
- POST messages `{peer_id,body}` -> Message
- POST messages/{id}/retry -> `{ok:true}`
- GET transfers -> Transfer[]
- POST transfers?peer_id= multipart `file` -> Transfer, streamed to local spool
- POST transfers/{id}/accept -> `{ok:true}`
- POST transfers/{id}/reject -> `{ok:true}`
- POST transfers/{id}/cancel -> `{ok:true}`
- POST transfers/{id}/retry -> `{ok:true}`
- GET settings -> `{download_dir:string,max_file_size:number}`
- PATCH settings `{max_file_size:number}` -> settings

Device = `{id:string,name:string,platform:string}`.
Peer = Device & `{addresses:string[],port:number,online:boolean,paired:boolean}`.
Pairing = `{id:string,peer_id:string,peer_name:string,code:string,local_confirmed:boolean,remote_confirmed:boolean,expires_at:number}`. Timestamp in epoch milliseconds.
Message = `{id:string,peer_id:string,direction:"incoming"|"outgoing",body:string,created_at:number,status:"sending"|"sent"|"pending"|"failed"}`.
Transfer = `{id:string,peer_id:string,direction:"incoming"|"outgoing",name:string,size:number,sha256:string,offset:number,status:"offered"|"awaiting_accept"|"transferring"|"paused"|"completed"|"rejected"|"cancelled"|"failed",error:string|null,created_at:number}`.
