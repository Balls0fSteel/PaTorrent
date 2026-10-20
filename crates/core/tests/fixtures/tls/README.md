# TLS test fixtures

Throwaway keys and self-signed certificates for the TLS layer's tests
(`crates/core/src/net/tls*`, `crates/nox/tests/https.rs`). They protect nothing; never use them
for a real server. Made with OpenSSL 3.5:

```sh
openssl ecparam -name prime256v1 -genkey -noout -out ec-sec1.pem
openssl pkcs8 -topk8 -nocrypt -in ec-sec1.pem -out ec-pkcs8.pem
openssl req -x509 -key ec-pkcs8.pem -out ec-cert.pem -subj "/CN=PaTorrent test" \
    -addext "subjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1" -addext "extendedKeyUsage=serverAuth" \
    -set_serial 4660 -not_before 20260101000000Z -not_after 20360101000000Z
openssl genrsa -traditional -out rsa-pkcs1.pem 2048
openssl req -x509 -key rsa-pkcs1.pem -out rsa-cert.pem -subj "/CN=PaTorrent test RSA" \
    -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" \
    -set_serial 4661 -not_before 20260101000000Z -not_after 20360101000000Z
openssl pkcs8 -topk8 -v2 aes-128-cbc -passout pass:x -in ec-pkcs8.pem -out ec-encrypted.pem
```

The EC key comes in three forms (SEC 1, PKCS #8, encrypted PKCS #8); the RSA key is PKCS #1.
`ec-cert.pem`'s SHA-256 fingerprint, which a test checks, is
`78:B1:27:3E:3E:EA:74:50:73:AD:12:CA:BC:20:04:88:A7:81:5B:08:FE:8C:95:CF:8F:0E:A2:4F:89:E0:7D:D5`.
