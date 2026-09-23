# Edge probe test certificates

A throwaway CA (`ca.der`) and a leaf for `edge.test` signed by it (`leaf.der`, key `leaf.key.der`,
PKCS#8), used only by `cloudflare.rs`'s tests to run a local TLS server the probe must reach by a
pinned address. `edge.test` is reserved (RFC 2606) and resolves nowhere. The key is public on
purpose and protects nothing; nothing outside the tests trusts this CA. Valid until 2126.

The leaf key is P-256 and must be a full 32-byte scalar: LibreSSL sometimes writes 31, which ring
refuses ("failed to parse private key").
