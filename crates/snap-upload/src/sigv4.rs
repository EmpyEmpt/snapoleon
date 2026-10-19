//! AWS Signature Version 4 (header-based `Authorization`), implemented
//! directly on `hmac` + `sha2`. Signs the real payload hash, which every
//! S3-compatible provider accepts (some reject `UNSIGNED-PAYLOAD`).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::util::pct;

type HmacSha256 = Hmac<Sha256>;

pub struct Credentials<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
}

/// Everything produced while signing; the intermediate strings are exposed so
/// callers can compare them with the AWS documentation vectors.
#[derive(Debug, Clone)]
pub struct Signed {
    pub canonical_request: String,
    pub string_to_sign: String,
    pub signature: String,
    pub signed_headers: String,
    /// Value for the `Authorization` header.
    pub authorization: String,
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Percent-encode a URI path the way S3 expects it (each segment once, `/` kept).
pub fn canonical_uri(path: &str) -> String {
    let encoded = path.split('/').map(pct).collect::<Vec<_>>().join("/");
    if encoded.starts_with('/') { encoded } else { format!("/{encoded}") }
}

/// Sign a request.
///
/// `headers` must contain every header that goes into the signature (at least
/// `host` and `x-amz-date`; for S3 also `x-amz-content-sha256`). `canonical_query`
/// is the already sorted and encoded query string. `amz_date` is
/// `YYYYMMDD'T'HHMMSS'Z'`.
#[allow(clippy::too_many_arguments)]
pub fn sign(
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    headers: &[(String, String)],
    payload_hash: &str,
    creds: &Credentials,
    region: &str,
    service: &str,
    amz_date: &str,
) -> Signed {
    let mut norm: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.split_whitespace().collect::<Vec<_>>().join(" ")))
        .collect();
    norm.sort_by(|a, b| a.0.cmp(&b.0));

    let canonical_headers: String = norm.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = norm.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let canonical_request = format!("{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}");

    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(canonical_request.as_bytes()));

    let k_date = hmac(format!("AWS4{}", creds.secret_key).as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!("AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed_headers},Signature={signature}", creds.access_key);
    Signed { canonical_request, string_to_sign, signature, signed_headers, authorization }
}
