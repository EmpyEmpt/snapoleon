//! Ready-made endpoint settings for common S3-compatible providers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct S3Preset {
    pub name: &'static str,
    /// May contain `{region}`, which is replaced by the destination's region.
    /// R2 needs the account id filled in (`<account-id>` is rejected until then).
    pub endpoint: &'static str,
    pub region: &'static str,
    pub path_style: bool,
}

const PRESETS: &[S3Preset] = &[
    S3Preset { name: "Amazon S3", endpoint: "https://s3.{region}.amazonaws.com", region: "us-east-1", path_style: false },
    S3Preset { name: "Cloudflare R2", endpoint: "https://<account-id>.r2.cloudflarestorage.com", region: "auto", path_style: true },
    S3Preset { name: "Yandex Object Storage", endpoint: "https://storage.yandexcloud.net", region: "ru-central1", path_style: false },
    S3Preset { name: "Selectel", endpoint: "https://s3.ru-1.storage.selcloud.ru", region: "ru-1", path_style: true },
    S3Preset { name: "VK Cloud", endpoint: "https://hb.ru-msk.vkcloud-solutions.ru", region: "ru-msk", path_style: true },
    S3Preset { name: "MinIO", endpoint: "http://localhost:9000", region: "us-east-1", path_style: true },
    S3Preset { name: "Backblaze B2", endpoint: "https://s3.us-west-004.backblazeb2.com", region: "us-west-004", path_style: false },
    S3Preset { name: "Wasabi", endpoint: "https://s3.wasabisys.com", region: "us-east-1", path_style: false },
    S3Preset { name: "DigitalOcean Spaces", endpoint: "https://nyc3.digitaloceanspaces.com", region: "nyc3", path_style: false },
];

pub fn s3() -> &'static [S3Preset] {
    PRESETS
}
