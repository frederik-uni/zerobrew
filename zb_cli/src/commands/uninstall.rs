use crate::ui::StdUi;
use crate::utils::normalize_formula_name;
use console::style;

pub fn execute(
    installer: &mut zb_io::Installer,
    formulas: Vec<String>,
    force: bool,
    all: bool,
    category: Option<String>,
    ui: &mut StdUi,
) -> Result<(), zb_core::Error> {
    if let Some(category) = category {
        let category = category.trim();
        let result = installer.uninstall_category(category, force)?;
        if result.requested.is_empty() {
            ui.info(format!(
                "No explicitly installed formulas in category '{category}'."
            ))
            .map_err(ui_error)?;
            return Ok(());
        }
        ui.heading(format!(
            "Uninstalled category {}: {}",
            style(category).bold(),
            result.requested.join(", ")
        ))
        .map_err(ui_error)?;
        for name in result.autoremoved {
            ui.info(format!("Removed unused dependency {name}"))
                .map_err(ui_error)?;
        }
        return Ok(());
    }

    let formulas = if all {
        let installed = installer.list_installed()?;
        if installed.is_empty() {
            ui.info("No formulas installed.").map_err(ui_error)?;
            return Ok(());
        }
        installed.into_iter().map(|k| k.name).collect()
    } else {
        let mut normalized = Vec::with_capacity(formulas.len());
        for formula in formulas {
            normalized.push(normalize_formula_name(&formula)?);
        }
        normalized
    };

    ui.heading(format!(
        "Uninstalling {}...",
        style(formulas.join(", ")).bold()
    ))
    .map_err(ui_error)?;

    let result = installer.uninstall_many(&formulas, force || all)?;
    for name in result.autoremoved {
        ui.info(format!("Removed unused dependency {name}"))
            .map_err(ui_error)?;
    }
    Ok(())
}

fn ui_error(err: std::io::Error) -> zb_core::Error {
    zb_core::Error::StoreCorruption {
        message: format!("failed to write CLI output: {err}"),
    }
}
