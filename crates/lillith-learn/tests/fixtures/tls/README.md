Throwaway test-only TLS chain for `tests/online_research.rs` (never used by
the app): `ca.der` is a self-made "Lillith Test CA" (P-256, not in any trust
store), `srv.der` is a leaf for `example.org` / `www.example.org` signed by
it, `srv.key.der` its PKCS#8 key. Valid until 2126. Regenerate with openssl
(`ecparam -name prime256v1`, `req -x509 … -extensions v3_ca`, `x509 -req …
-extensions v3_srv` with `subjectAltName=DNS:example.org`, then `-outform DER`).
