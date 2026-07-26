# Vendored OpenCodeX proxy core

The files in `opencodex_proxy/` are copied from
[`booboom/opencodex`](https://github.com/booboom/opencodex) at commit
`24379c8655a4c20840e9d8670b7451c8c0cb9a6f`.

The desktop application launches this original Python implementation as a frozen
sidecar. `app.py` and `protocol.py` retain the upstream Responses ↔ Chat Completions
conversion, streaming, tool-call, reasoning replay, model routing and safety logic.
The MIT license is preserved in `UPSTREAM_LICENSE`.
