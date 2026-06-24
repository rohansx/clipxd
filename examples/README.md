# Examples

## `checkout-500.trace.json` — run the browser backend with **only Rust**

No ffmpeg / tesseract / Node needed — browser ingest is pure Rust.

```bash
cargo build                                   # or: cargo build --release
clipxd=target/debug/clipxd                    # release: target/release/clipxd

$clipxd ingest-browser examples/checkout-500.trace.json --out /tmp/clipxd-demo
clip=$(echo /tmp/clipxd-demo/clp_*)

$clipxd query  "$clip" "what error showed up and what was the user doing right before it"
$clipxd search "$clip" "payment failed 500"
$clipxd info   "$clip"
cat "$clip/index.json"                          # the artifact an agent queries
```

Expected `query` answer: *"…'POST /api/checkout 500 Internal Server Error'. Just before:
clicked 'Place order'"* — answered from the index, never from pixels.

To capture your **own** browser trace, see [`../tools/clipxd-capture/`](../tools/clipxd-capture/).
For the **import** path (any video → index, needs ffmpeg + tesseract `eng.traineddata`), see
`clipxd import --help`.
