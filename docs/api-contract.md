# Local API contract

All routes use `/api/v1`. JSON response arrays are bare arrays. Errors use `{ "error": "human readable reason" }`. UI and API share the exact loopback origin. Mutations require `X-LANLink-Token` from `GET /api/v1/session` and exact Origin. GET session requires same-origin fetch header. WS supplies token using `Sec-WebSocket-Protocol` values `lanlink` and token. WS sends `{ "type": "changed" }` to invalidate caches and clients refetch after reconnect.

- GET health -> `{service:"lanlink",status:"ok",version:string,update_failed_version?:string}`. After a failed upgrade, the optional field identifies the failed candidate.
- POST updates/check -> `{status:"current"}`, `{status:"available",version:string}`, or `{status:"unsupported"}`. An unavailable or invalid official release index returns HTTP 502.
- POST updates/apply `{version:string}` -> `{version:string}`. The authenticated request downloads and verifies the published package and returns once a helper takes over. The browser polls health on the same origin and reloads after it sees the requested version.
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
Transfer = `{id:string,peer_id:string,direction:"incoming"|"outgoing",name:string,size:number,sha256:string,offset:number,status:"offered"|"awaiting_accept"|"transferring"|"paused"|"completed"|"rejected"|"cancelled"|"failed",error:string|null,failure_kind?:"source_unavailable"|null,created_at:number}`.

## Browser file transfer

The desktop API opens one temporary HTTP listener on a selected LAN IPv4 address. The desktop routes use `/api/v1`. The temporary listener serves only the `/mobile/` page, its assets and its file transfer API.

| Desktop route            | Request                | Response                                                          |
| ------------------------ | ---------------------- | ----------------------------------------------------------------- |
| `GET /mobile`            | `X-LANLink-Token`      | Interface addresses and current session                           |
| `POST /mobile`           | `{address}`            | Starts a session and returns the desktop state                    |
| `POST /mobile/approve`   | `{code,remember?}`     | Approves a matching pending session and returns the desktop state |
| `DELETE /mobile`         |                        | Ends the session and returns the desktop state                    |
| `POST /mobile/files`     | Multipart `file`       | Metadata for the file available to the mobile browser             |
| `GET /mobile/files/{id}` | Same-origin navigation | Attachment for a file received from the mobile browser            |

Desktop state is `{addresses,session,remembered_browsers}`. `session` is null or includes `url` and the mobile state fields. The QR URL carries the one-use invite in its fragment. Mobile state includes `{phase,code,expires_at,max_file_size,files,remembered,remembered_browser_id}`. `expires_at` is epoch milliseconds and `max_file_size` is in bytes. The phases are `inviting`, `pending` and `approved`. Each file is `{id,name,size,direction}`, with direction `to_computer` or `to_mobile`.

On the temporary listener, `POST /mobile/claim` with `{invite}` establishes a browser credential as a cookie and returns mobile state. `GET /mobile/state` reads that state using the cookie. After desktop approval, `POST /mobile/files` accepts multipart `file` and `GET /mobile/files/{id}` serves an attachment staged for the mobile browser. Mobile writes require the temporary listener's exact Origin. Ending the session or reaching its expiry invalidates the credential.

Approval remembers the browser by default. Set `remember` to false for a temporary guest. The authenticated mobile state response issues a separate host-only HttpOnly cookie for future scans. The registry persists only the credential hash and metadata. A remembered browser must present both its valid credential and a fresh invitation to become approved in a new session. Successful recall renews the 90-day authorization. Clearing browser data, switching browser profiles or changing the desktop's LAN IP can require approval again.

`remembered_browsers` contains `{id,name,created_at,last_used_at,expires_at}` entries. `DELETE /api/v1/mobile/browsers/{id}` revokes the stored browser and ends its active transfer session, then returns desktop state. Ending a temporary transfer session preserves the browser's remembered authorization.
