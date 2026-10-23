use std::path::Path;
use std::sync::atomic::AtomicBool;

pub fn enabled() -> bool {
    cfg!(feature = "direct-drive")
}

pub async fn upload_export(
    project: &Path,
    progress: impl Fn(f64),
    cancel: &AtomicBool,
) -> Result<crate::upload::UploadResult, String> {
    let meta = cap_project::RecordingMeta::load_for_project(project)
        .map_err(|_| "Could not read the recording metadata")?;
    let link =
        cap_direct_drive::upload(&meta.output_path(), &meta.pretty_name, progress, cancel).await?;
    Ok(crate::upload::UploadResult::Success(link))
}
