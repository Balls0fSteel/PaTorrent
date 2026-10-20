// PaTorrent's own code, not ported: libtorrent hands certificate files to OpenSSL as they are.

//! PEM files, and the little DER the TLS layer reads or writes itself: a certificate's validity
//! period and fingerprint, and (for CNG, which imports PKCS #8 only) PKCS #1 and SEC 1 private
//! keys wrapped into PKCS #8.

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
#[cfg(any(windows, test))]
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
#[cfg(any(windows, test))]
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
#[cfg(any(windows, test))]
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
    fn encode_round_trips() {
        let cert = certificates(RSA_CERT).unwrap().remove(0);
        let text = encode("CERTIFICATE", &cert);
        assert!(text.lines().all(|l| l.len() <= 64));
        assert_eq!(certificates(text.as_bytes()).unwrap(), vec![cert]);
        assert_eq!(der(0x04, &[0; 200])[..3], [0x04, 0x81, 200]);
        assert_eq!(der(0x04, &[0; 300])[..4], [0x04, 0x82, 0x01, 0x2c]);
    }
}
