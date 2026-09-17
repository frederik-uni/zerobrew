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
                ownership_label(
                    formula.keg.explicit,
                    formula.keg.explicit_category.as_deref(),
                    &formula.required_by,
                )
            );
        }
    }

    Ok(())
}

fn ownership_label(
    explicit: bool,
    explicit_category: Option<&str>,
    required_by: &[String],
) -> String {
    if !explicit {
        return if required_by.is_empty() {
            "implicit (orphan)".to_string()
        } else {
            format!("implicit (required by {})", required_by.join(", "))
        };
    }

    let mut label = match explicit_category {
        Some(category) => format!("explicit [{category}]"),
        None => "explicit".to_string(),
    };
    if !required_by.is_empty() {
        label.push_str(&format!(" (also required by {})", required_by.join(", ")));
    }
    label
}

#[cfg(test)]
mod tests {
    use super::ownership_label;

    #[test]
    fn labels_all_ownership_states() {
        assert_eq!(ownership_label(true, None, &[]), "explicit");
        assert_eq!(
            ownership_label(true, None, &["ffmpeg".into(), "wget".into()]),
            "explicit (also required by ffmpeg, wget)"
        );
        assert_eq!(
            ownership_label(false, None, &["ffmpeg".into()]),
            "implicit (required by ffmpeg)"
        );
        assert_eq!(ownership_label(false, None, &[]), "implicit (orphan)");
        assert_eq!(
            ownership_label(true, Some("experiment-a"), &[]),
            "explicit [experiment-a]"
        );
        assert_eq!(
            ownership_label(true, Some("experiment-a"), &["suite".into()]),
            "explicit [experiment-a] (also required by suite)"
        );
    }
}
