use console::style;

pub fn execute(installer: &mut zb_io::Installer) -> Result<(), zb_core::Error> {
    let installed = installer.list_installed_with_ownership()?;

    if installed.is_empty() {
        println!("No formulas installed.");
    } else {
        for formula in installed {
            println!(
                "{} {} {}",
                style(&formula.keg.name).bold(),
                style(&formula.keg.version).dim(),
                ownership_label(formula.keg.explicit, &formula.required_by)
            );
        }
    }

    Ok(())
}

fn ownership_label(explicit: bool, required_by: &[String]) -> String {
    match (explicit, required_by.is_empty()) {
        (true, true) => "explicit".to_string(),
        (true, false) => format!("explicit (also required by {})", required_by.join(", ")),
        (false, false) => format!("implicit (required by {})", required_by.join(", ")),
        (false, true) => "implicit (orphan)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::ownership_label;

    #[test]
    fn labels_all_ownership_states() {
        assert_eq!(ownership_label(true, &[]), "explicit");
        assert_eq!(
            ownership_label(true, &["ffmpeg".into(), "wget".into()]),
            "explicit (also required by ffmpeg, wget)"
        );
        assert_eq!(
            ownership_label(false, &["ffmpeg".into()]),
            "implicit (required by ffmpeg)"
        );
        assert_eq!(ownership_label(false, &[]), "implicit (orphan)");
    }
}
