use crate::ui::StdUi;

pub fn execute(installer: &mut zb_io::Installer, ui: &mut StdUi) -> Result<(), zb_core::Error> {
    let removed = installer.autoremove()?;
    if removed.is_empty() {
        ui.info("No unused dependencies.".to_string())
            .map_err(ui_error)?;
    } else {
        ui.info(format!(
            "Removed unused dependencies: {}",
            removed.join(", ")
        ))
        .map_err(ui_error)?;
    }
    Ok(())
}

fn ui_error(err: std::io::Error) -> zb_core::Error {
    zb_core::Error::StoreCorruption {
        message: format!("failed to write CLI output: {err}"),
    }
}
