// PaTorrent's own code, not ported: libtorrent hands certificate files to OpenSSL as they are.

//! PEM files, and the little DER the TLS layer reads or writes itself: a certificate's validity
//! period and fingerprint, (for CNG, which imports PKCS #8 only) PKCS #1 and SEC 1 private keys
//! wrapped into PKCS #8, and (for Security.framework, which reads raw keys and makes no
//! certificates) the parts of EC keys and a self-signed certificate built around a signature.

use std::io;

use crate::util::string::base64decode;

/// One `-----BEGIN <label>-----` block, decoded.
pub(crate) struct Block {
    pub label: String,
    pub der: Vec<u8>,
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Every PEM block in `text`, in order. Text outside the blocks is ignored, as OpenSSL does, so
/// files with a human-readable dump in front of the block load too.
pub(crate) fn parse(text: &[u8]) -> io::Result<Vec<Block>> {
    let text =
        std::str::from_utf8(text).map_err(|_| invalid("not a PEM file (DER files are not supported)"))?;
    let mut blocks = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(label) = line.trim().strip_prefix("-----BEGIN ").and_then(|l| l.strip_suffix("-----"))
        else {
            continue;
        };
        let end = format!("-----END {label}-----");
        let (mut body, mut encrypted, mut closed) = (String::new(), false, false);
        for line in lines.by_ref() {
            let line = line.trim();
            if line == end {
                closed = true;
                break;
            }
            // RFC 1421 headers: OpenSSL's legacy encrypted keys say `Proc-Type: 4,ENCRYPTED`
            if line.contains(':') {
                encrypted |= line.starts_with("Proc-Type:") && line.contains("ENCRYPTED");
                continue;
            }
            body.push_str(line);
        }
        if !closed {
            return Err(invalid(format!("the PEM block {label} has no end line")));
        }
        if encrypted {
            return Err(invalid(ENCRYPTED));
        }
        let der = base64decode(body.as_bytes())
            .ok_or_else(|| invalid(format!("the PEM block {label} is corrupt")))?;
        blocks.push(Block { label: label.to_owned(), der });
    }
    Ok(blocks)
}

const ENCRYPTED: &str =
    "the private key is encrypted; store it without a passphrase (openssl pkey -in key.pem -out plain.pem)";

/// `der` as a PEM block, 64 columns, with a final line break.
#[cfg(any(windows, target_vendor = "apple", test))]
pub(crate) fn encode(label: &str, der: &[u8]) -> String {
    let b64 = crate::util::string::base64encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// The certificates in a PEM file (the server's own first, then its chain), as DER.
pub(crate) fn certificates(text: &[u8]) -> io::Result<Vec<Vec<u8>>> {
    let certs: Vec<Vec<u8>> =
        parse(text)?.into_iter().filter(|b| b.label == "CERTIFICATE").map(|b| b.der).collect();
    if certs.is_empty() {
        return Err(invalid("no certificate (-----BEGIN CERTIFICATE-----) in the file"));
    }
    if validity(&certs[0]).is_none() {
        return Err(invalid("the certificate is corrupt"));
    }
    Ok(certs)
}

/// The first private key in a PEM file: PKCS #8, PKCS #1 (RSA) or SEC 1 (EC).
pub(crate) fn private_key(text: &[u8]) -> io::Result<Block> {
    for b in parse(text)? {
        match b.label.as_str() {
            "PRIVATE KEY" | "RSA PRIVATE KEY" | "EC PRIVATE KEY" => return Ok(b),
            "ENCRYPTED PRIVATE KEY" => return Err(invalid(ENCRYPTED)),
            _ => {}
        }
    }
    Err(invalid("no private key (-----BEGIN PRIVATE KEY-----) in the file"))
}

// --- DER -------------------------------------------------------------------------------------

/// One DER element: its tag, contents, and what follows it. Definite lengths only.
pub(crate) fn tlv(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, mut rest) = rest.split_first()?;
    let len = if first < 0x80 {
        first as usize
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n].iter().fold(0usize, |l, &b| l << 8 | b as usize);
        rest = &rest[n..];
        len
    };
    (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
}

/// A certificate's validity period, `(notBefore, notAfter)` in Unix seconds.
pub(crate) fn validity(cert: &[u8]) -> Option<(i64, i64)> {
    let Some((0x30, cert, _)) = tlv(cert) else { return None };
    let Some((0x30, tbs, _)) = tlv(cert) else { return None };
    // version [0] (optional), serialNumber, signature, issuer, validity
    let (tag, _, mut rest) = tlv(tbs)?;
    if tag == 0xa0 {
        rest = tlv(rest)?.2;
    }
    let rest = tlv(tlv(rest)?.2)?.2;
    let Some((0x30, validity, _)) = tlv(rest) else { return None };
    let (t1, v1, rest) = tlv(validity)?;
    let (t2, v2, _) = tlv(rest)?;
    Some((time(t1, v1)?, time(t2, v2)?))
}

/// UTCTime (`YYMMDDHHMMSSZ`) or GeneralizedTime (`YYYYMMDDHHMMSSZ`), as DER requires them.
fn time(tag: u8, v: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(v).ok()?.strip_suffix('Z')?;
    let (year, s) = match tag {
        0x17 => {
            let yy: i64 = s.get(..2)?.parse().ok()?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, s.get(2..)?)
        }
        0x18 => (s.get(..4)?.parse().ok()?, s.get(4..)?),
        _ => return None,
    };
    if s.len() != 10 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |i: usize| s[i..i + 2].parse::<i64>().expect("digits");
    let (month, day, h, m, sec) = (n(0), n(2), n(4), n(6), n(8));
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || h > 23 || m > 59 || sec > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86400 + h * 3600 + m * 60 + sec)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day number from [`days_from_civil`]: `(year, month, day)`.
#[cfg(any(windows, target_vendor = "apple", test))]
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A DER element with `content`.
#[cfg(any(windows, target_vendor = "apple", test))]
pub(crate) fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = content.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes = n.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(content);
    out
}

/// The key as a PKCS #8 `PrivateKeyInfo`: PKCS #1 and SEC 1 keys get the wrapper PKCS #8 puts
/// around them (RFC 5208, RFC 5915).
#[cfg(any(windows, test))]
pub(crate) fn pkcs8(key: &Block) -> io::Result<Vec<u8>> {
    /// rsaEncryption (1.2.840.113549.1.1.1) and its NULL parameters
    const RSA: &[u8] = &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00];
    /// id-ecPublicKey (1.2.840.10045.2.1)
    const EC: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
    let algorithm = match key.label.as_str() {
        "PRIVATE KEY" => return Ok(key.der.clone()),
        "RSA PRIVATE KEY" => RSA.to_vec(),
        "EC PRIVATE KEY" => {
            let curve = ec_curve(&key.der).ok_or_else(|| invalid("the EC private key names no curve"))?;
            [EC, curve].concat()
        }
        other => return Err(invalid(format!("unsupported private key type {other}"))),
    };
    let body = [der(0x02, &[0]), der(0x30, &algorithm), der(0x04, &key.der)].concat();
    Ok(der(0x30, &body))
}

/// The curve OID (as a whole element) in a SEC 1 `ECPrivateKey`'s `[0] parameters`.
#[cfg(any(windows, test))]
fn ec_curve(sec1: &[u8]) -> Option<&[u8]> {
    let Some((0x30, mut fields, _)) = tlv(sec1) else { return None };
    while !fields.is_empty() {
        let (tag, content, rest) = tlv(fields)?;
        if tag == 0xa0 {
            let (0x06, _, _) = tlv(content)? else { return None };
            return Some(content);
        }
        fields = rest;
    }
    None
}

/// rsaEncryption (1.2.840.113549.1.1.1), an OID's contents.
#[cfg(any(target_vendor = "apple", test))]
pub(crate) const RSA_OID: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
/// id-ecPublicKey (1.2.840.10045.2.1), an OID's contents.
#[cfg(any(target_vendor = "apple", test))]
pub(crate) const EC_OID: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];

/// A PKCS #8 `PrivateKeyInfo`'s algorithm (the OID's contents) and the key it wraps (PKCS #1
/// for RSA, SEC 1 for EC).
#[cfg(any(target_vendor = "apple", test))]
pub(crate) fn pkcs8_key(info: &[u8]) -> Option<(&[u8], &[u8])> {
    let Some((0x30, body, _)) = tlv(info) else { return None };
    let Some((0x02, _, rest)) = tlv(body) else { return None };
    let Some((0x30, algorithm, rest)) = tlv(rest) else { return None };
    let Some((0x06, oid, _)) = tlv(algorithm) else { return None };
    let Some((0x04, key, _)) = tlv(rest) else { return None };
    Some((oid, key))
}

/// A SEC 1 `ECPrivateKey`'s private scalar and, when the key carries it, its public point
/// (`04 || X || Y`).
#[cfg(any(target_vendor = "apple", test))]
pub(crate) fn ec_private_key(sec1: &[u8]) -> Option<(&[u8], Option<&[u8]>)> {
    let Some((0x30, body, _)) = tlv(sec1) else { return None };
    let Some((0x02, &[1], rest)) = tlv(body) else { return None };
    let Some((0x04, d, mut rest)) = tlv(rest) else { return None };
    let mut public = None;
    while !rest.is_empty() {
        let (tag, content, next) = tlv(rest)?;
        if tag == 0xa1 {
            let Some((0x03, bits, _)) = tlv(content) else { return None };
            public = bit_string(bits);
        }
        rest = next;
    }
    Some((d, public))
}

/// A BIT STRING's bytes, when it is a whole number of them.
#[cfg(any(target_vendor = "apple", test))]
fn bit_string(contents: &[u8]) -> Option<&[u8]> {
    match contents.split_first() {
        Some((0, bytes)) => Some(bytes),
        _ => None,
    }
}

/// A certificate's `subjectPublicKey` (for an EC key, its point `04 || X || Y`).
#[cfg(any(target_vendor = "apple", test))]
pub(crate) fn subject_public_key(cert: &[u8]) -> Option<&[u8]> {
    let Some((0x30, cert, _)) = tlv(cert) else { return None };
    let Some((0x30, tbs, _)) = tlv(cert) else { return None };
    // version [0] (optional), serialNumber, signature, issuer, validity, subject, then the key
    let (tag, _, mut rest) = tlv(tbs)?;
    for _ in 0..if tag == 0xa0 { 5 } else { 4 } {
        rest = tlv(rest)?.2;
    }
    let Some((0x30, spki, _)) = tlv(rest) else { return None };
    let Some((0x03, bits, _)) = tlv(tlv(spki)?.2) else { return None };
    bit_string(bits)
}

/// A UTCTime for the years 1950 to 2049 and a GeneralizedTime otherwise, as RFC 5280 asks.
#[cfg(any(target_vendor = "apple", test))]
pub(crate) fn der_time(unix: i64) -> Vec<u8> {
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    let rest = format!("{m:02}{d:02}{:02}{:02}{:02}Z", secs / 3600, secs / 60 % 60, secs % 60);
    if (1950..2050).contains(&y) {
        der(0x17, format!("{:02}{rest}", y % 100).as_bytes())
    } else {
        der(0x18, format!("{y:04}{rest}").as_bytes())
    }
}

/// The algorithms of a self-signed P-256 certificate, as whole elements: ecdsa-with-SHA256
/// (1.2.840.10045.4.3.2) and prime256v1 (1.2.840.10045.3.1.7).
#[cfg(any(target_vendor = "apple", test))]
const ECDSA_SHA256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
#[cfg(any(target_vendor = "apple", test))]
const P256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];

/// `AlgorithmIdentifier { id-ecPublicKey, prime256v1 }`.
#[cfg(any(target_vendor = "apple", test))]
fn p256_algorithm() -> Vec<u8> {
    der(0x30, &[&der(0x06, EC_OID), P256].concat())
}

/// The `TBSCertificate` of an X.509 v3 certificate that `cn` issues to itself for `names`, for
/// the P-256 public key `point` (`04 || X || Y`), to be signed with ecdsa-with-SHA256; the
/// extensions are subjectAltName and extendedKeyUsage serverAuth, as the other backends make
/// them. `serial` must be a positive DER integer's bytes.
#[cfg(any(target_vendor = "apple", test))]
pub(crate) fn self_signed_tbs(
    cn: &str,
    names: &[super::SubjectName],
    serial: &[u8],
    not_before: i64,
    not_after: i64,
    point: &[u8],
) -> Vec<u8> {
    use super::SubjectName;
    // Name ::= SEQUENCE OF SET OF { commonName (2.5.4.3), UTF8String }
    let cn_attr = [&[0x06, 0x03, 0x55, 0x04, 0x03][..], &der(0x0c, cn.as_bytes())].concat();
    let name = der(0x30, &der(0x31, &der(0x30, &cn_attr)));
    // subjectAltName: dNSName [2] and iPAddress [7]
    let alt: Vec<u8> = names
        .iter()
        .flat_map(|n| match n {
            SubjectName::Dns(d) => der(0x82, d.as_bytes()),
            SubjectName::Ip(std::net::IpAddr::V4(ip)) => der(0x87, &ip.octets()),
            SubjectName::Ip(std::net::IpAddr::V6(ip)) => der(0x87, &ip.octets()),
        })
        .collect();
    // extKeyUsage: id-kp-serverAuth (1.3.6.1.5.5.7.3.1)
    let eku = der(0x30, &[0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01]);
    let extension = |oid: &[u8], value: &[u8]| der(0x30, &[&der(0x06, oid), &der(0x04, value)[..]].concat());
    let extensions = [extension(&[0x55, 0x1d, 0x11], &der(0x30, &alt)), extension(&[0x55, 0x1d, 0x25], &eku)];
    let spki = der(0x30, &[p256_algorithm(), der(0x03, &[&[0], point].concat())].concat());
    der(
        0x30,
        &[
            // version v3
            der(0xa0, &der(0x02, &[2])),
            der(0x02, serial),
            der(0x30, ECDSA_SHA256),
            name.clone(),
            der(0x30, &[der_time(not_before), der_time(not_after)].concat()),
            name,
            spki,
            der(0xa3, &der(0x30, &extensions.concat())),
        ]
        .concat(),
    )
}

/// The certificate made of a [`self_signed_tbs`] and its ECDSA signature (a DER
/// `Ecdsa-Sig-Value`).
#[cfg(any(target_vendor = "apple", test))]
pub(crate) fn self_signed_certificate(tbs: &[u8], signature: &[u8]) -> Vec<u8> {
    der(0x30, &[tbs, &der(0x30, ECDSA_SHA256), &der(0x03, &[&[0], signature].concat())].concat())
}

/// A P-256 private key as PKCS #8, the way OpenSSL writes it: the curve in the algorithm, the
/// public point inside the `ECPrivateKey`.
#[cfg(any(target_vendor = "apple", test))]
pub(crate) fn p256_pkcs8(d: &[u8], point: &[u8]) -> Vec<u8> {
    let ec_key =
        der(0x30, &[der(0x02, &[1]), der(0x04, d), der(0xa1, &der(0x03, &[&[0], point].concat()))].concat());
    der(0x30, &[der(0x02, &[0]), p256_algorithm(), der(0x04, &ec_key)].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EC_CERT: &[u8] = include_bytes!("../../../tests/fixtures/tls/ec-cert.pem");
    const EC_SEC1: &[u8] = include_bytes!("../../../tests/fixtures/tls/ec-sec1.pem");
    const EC_PKCS8: &[u8] = include_bytes!("../../../tests/fixtures/tls/ec-pkcs8.pem");
    const EC_ENCRYPTED: &[u8] = include_bytes!("../../../tests/fixtures/tls/ec-encrypted.pem");
    const RSA_CERT: &[u8] = include_bytes!("../../../tests/fixtures/tls/rsa-cert.pem");
    const RSA_PKCS1: &[u8] = include_bytes!("../../../tests/fixtures/tls/rsa-pkcs1.pem");

    #[test]
    fn certificates_and_their_validity() {
        // both fixtures: notBefore 2026-01-01, notAfter 2036-01-01, as UTCTime
        let from = days_from_civil(2026, 1, 1) * 86400;
        let to = days_from_civil(2036, 1, 1) * 86400;
        for pem in [EC_CERT, RSA_CERT] {
            let certs = certificates(pem).unwrap();
            assert_eq!(certs.len(), 1);
            assert_eq!(validity(&certs[0]), Some((from, to)));
        }
        assert_eq!(from, 1_767_225_600);
        // GeneralizedTime from 2050 on
        assert_eq!(time(0x18, b"20500101000000Z"), Some(days_from_civil(2050, 1, 1) * 86400));
        assert_eq!(time(0x17, b"491231235959Z"), Some(days_from_civil(2050, 1, 1) * 86400 - 1));
        assert_eq!(time(0x17, b"9912312359Z"), None);
        assert_eq!(time(0x17, b"991332235959Z"), None);
        for days in [-1, 0, 59, 10_957, 20_746, 24_837] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m as i64, d as i64), days);
        }
    }

    #[test]
    fn a_chain_keeps_its_order_and_text_around_blocks_is_skipped() {
        let mut file = b"subject=CN = leaf\r\n".to_vec();
        file.extend_from_slice(EC_CERT);
        file.extend_from_slice(b"\r\nintermediate:\r\n");
        file.extend_from_slice(RSA_CERT);
        let certs = certificates(&file).unwrap();
        assert_eq!(certs.len(), 2);
        assert_eq!(certs[0], certificates(EC_CERT).unwrap()[0]);
        assert_eq!(certs[1], certificates(RSA_CERT).unwrap()[0]);
    }

    #[test]
    fn keys_of_every_supported_form() {
        // PrivateKeyInfo ::= SEQUENCE { version, privateKeyAlgorithm, privateKey OCTET STRING }
        fn fields(info: &[u8]) -> (Vec<u8>, Vec<u8>) {
            let (tag, body, rest) = tlv(info).unwrap();
            assert_eq!((tag, rest), (0x30, &[][..]));
            let (_, version, body) = tlv(body).unwrap();
            assert_eq!(version, [0]);
            let (_, algorithm, body) = tlv(body).unwrap();
            let (tag, key, rest) = tlv(body).unwrap();
            assert_eq!((tag, rest), (0x04, &[][..]));
            (algorithm.to_vec(), key.to_vec())
        }
        let ec = private_key(EC_PKCS8).unwrap();
        assert_eq!(ec.label, "PRIVATE KEY");
        assert_eq!(pkcs8(&ec).unwrap(), ec.der);
        let sec1 = private_key(EC_SEC1).unwrap();
        assert_eq!(sec1.label, "EC PRIVATE KEY");
        // the same key as OpenSSL converted it: the same algorithm (with the curve) around it
        let (ours, key) = fields(&pkcs8(&sec1).unwrap());
        assert_eq!(ours, fields(&ec.der).0);
        assert_eq!(key, sec1.der);
        let rsa = private_key(RSA_PKCS1).unwrap();
        assert_eq!(rsa.label, "RSA PRIVATE KEY");
        let (algorithm, key) = fields(&pkcs8(&rsa).unwrap());
        assert_eq!(algorithm[..11], [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01]);
        assert_eq!(key, rsa.der);
    }

    #[test]
    fn unusable_files_say_why() {
        let msg = |r: io::Result<Block>| r.err().unwrap().to_string();
        assert!(msg(private_key(EC_ENCRYPTED)).contains("encrypted"));
        let legacy = b"-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nAAAA\n-----END RSA PRIVATE KEY-----\n";
        assert!(msg(private_key(legacy)).contains("encrypted"));
        assert!(msg(private_key(EC_CERT)).contains("no private key"));
        assert!(certificates(EC_PKCS8).unwrap_err().to_string().contains("no certificate"));
        assert!(
            certificates(b"-----BEGIN CERTIFICATE-----\nAAAA\n").unwrap_err().to_string().contains("no end")
        );
        assert!(certificates(b"-----BEGIN CERTIFICATE-----\nA*AA\n-----END CERTIFICATE-----").is_err());
        assert!(certificates(b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----").is_err());
        assert!(parse(&[0x30, 0x82, 0xff, 0xfe]).is_err());
    }

    #[test]
    fn the_parts_of_ec_and_rsa_keys() {
        let point = subject_public_key(&certificates(EC_CERT).unwrap()[0]).unwrap().to_vec();
        assert_eq!((point.len(), point[0]), (65, 4));
        // the SEC 1 file and the PKCS #8 one both belong to the EC certificate and carry its point
        let sec1 = private_key(EC_SEC1).unwrap();
        let (d, public) = ec_private_key(&sec1.der).unwrap();
        assert_eq!((d.len(), public), (32, Some(&point[..])));
        let ec8 = private_key(EC_PKCS8).unwrap();
        let (oid, inner) = pkcs8_key(&ec8.der).unwrap();
        assert_eq!(oid, EC_OID);
        let (d8, public) = ec_private_key(inner).unwrap();
        assert_eq!((d8.len(), public), (32, Some(&point[..])));
        // wrapped as OpenSSL wraps it, the key reads back the same
        let wrapped = p256_pkcs8(d, &point);
        let (oid, inner) = pkcs8_key(&wrapped).unwrap();
        assert_eq!((oid, ec_private_key(inner).unwrap()), (EC_OID, (d, Some(&point[..]))));
        assert_eq!(pkcs8(&Block { label: "PRIVATE KEY".into(), der: wrapped.clone() }).unwrap(), wrapped);
        // an RSA certificate's key is no EC point; a PKCS #8 RSA key wraps the PKCS #1 one
        let rsa_cert = certificates(RSA_CERT).unwrap().remove(0);
        assert_eq!(subject_public_key(&rsa_cert).unwrap()[0], 0x30);
        let rsa = private_key(RSA_PKCS1).unwrap();
        let info = pkcs8(&rsa).unwrap();
        assert_eq!(pkcs8_key(&info), Some((RSA_OID, &rsa.der[..])));
        assert_eq!(ec_private_key(&rsa.der), None);
        assert_eq!(pkcs8_key(&rsa.der), None);
    }

    #[test]
    fn times_switch_to_generalized_time_outside_1950_to_2049() {
        for (unix, tag) in [
            (days_from_civil(2026, 10, 21) * 86400 + 3723, 0x17),
            (days_from_civil(2049, 12, 31) * 86400 + 86399, 0x17),
            (days_from_civil(2050, 1, 1) * 86400, 0x18),
            (days_from_civil(1949, 12, 31) * 86400, 0x18),
            (days_from_civil(1950, 1, 1) * 86400, 0x17),
        ] {
            let el = der_time(unix);
            let (t, v, rest) = tlv(&el).unwrap();
            assert_eq!((t, rest), (tag, &[][..]));
            assert_eq!(time(t, v), Some(unix), "{}", String::from_utf8_lossy(v));
        }
        assert_eq!(der_time(days_from_civil(2026, 10, 21) * 86400 + 3723), der(0x17, b"261021010203Z"));
    }

    #[test]
    fn a_self_signed_certificate_reads_back() {
        let point = subject_public_key(&certificates(EC_CERT).unwrap()[0]).unwrap().to_vec();
        let (from, to) = (days_from_civil(2026, 10, 20) * 86400, days_from_civil(2051, 1, 1) * 86400);
        let names = [
            super::super::SubjectName::Dns("localhost".into()),
            super::super::SubjectName::Ip("127.0.0.1".parse().unwrap()),
            super::super::SubjectName::Ip("::1".parse().unwrap()),
        ];
        let tbs = self_signed_tbs("localhost", &names, &[0x42, 1, 2, 3, 4, 5, 6, 7], from, to, &point);
        let cert = self_signed_certificate(&tbs, &der(0x30, &[der(0x02, &[1]), der(0x02, &[2])].concat()));
        assert_eq!(validity(&cert), Some((from, to)));
        assert_eq!(subject_public_key(&cert), Some(&point[..]));
        let text = encode("CERTIFICATE", &cert);
        assert_eq!(certificates(text.as_bytes()).unwrap(), vec![cert.clone()]);
        // the names are in the subjectAltName extension
        assert!(cert.windows(11).any(|w| w == [&[0x82, 9][..], b"localhost"].concat()));
        assert!(cert.windows(6).any(|w| w == [0x87, 4, 127, 0, 0, 1]));
        assert!(cert.windows(18).any(|w| w[..2] == [0x87, 16] && w[17] == 1));
    }

    #[test]
    fn encode_round_trips() {
        let cert = certificates(RSA_CERT).unwrap().remove(0);
        let text = encode("CERTIFICATE", &cert);
        assert!(text.lines().all(|l| l.len() <= 64));
        assert_eq!(certificates(text.as_bytes()).unwrap(), vec![cert]);
        assert_eq!(der(0x04, &[0; 200])[..3], [0x04, 0x81, 200]);
        assert_eq!(der(0x04, &[0; 300])[..4], [0x04, 0x82, 0x01, 0x2c]);
    }
}
