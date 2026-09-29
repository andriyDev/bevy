---
title: LoadContext::read_asset_bytes replaced with LoadContext::read_asset
pull_requests: []
---

Previously, `LoadContext::read_asset_bytes` would load the path and read all
its bytes. This has been replaced by `LoadContext::read_asset` which loads the
path and returns you the reader so you can poll it.

If you would like to keep the old behavior:

```rust
// Before:
let bytes: Vec<u8> = load_context.read_asset_bytes("some_path").await?;

// After:
let mut reader = load_context.read_asset("some_path").await?;
let mut bytes = vec![];
reader.read_to_end(&mut bytes).await?;
```
