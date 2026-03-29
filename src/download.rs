use crate::telegram::TelegramClient;
use base64::Engine;

pub struct AttachedFile {
    pub mime: String,
    pub filename: String,
    pub data_url: String,
}

pub async fn download_telegram_file(
    client: &TelegramClient,
    file_id: &str,
    mime: &str,
    filename: &str,
) -> Option<AttachedFile> {
    let file = client.get_file(file_id).await.ok()?;
    let file_path = file.file_path?;
    let bytes = client.download_file_bytes(&file_path).await.ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let data_url = format!("data:{};base64,{}", mime, b64);
    Some(AttachedFile {
        mime: mime.to_string(),
        filename: filename.to_string(),
        data_url,
    })
}
