use std::collections::HashSet;

use zb_core::{Error, formula_token};

use super::{Installer, UninstallResult};

impl Installer {
    pub fn uninstall(&mut self, name: &str) -> Result<(), Error> {
        self.uninstall_many(&[name.to_string()], false).map(|_| ())
    }

    pub fn uninstall_many(
        &mut self,
        names: &[String],
        force: bool,
    ) -> Result<UninstallResult, Error> {
        let mut seen = HashSet::new();
        let requested: Vec<String> = names
            .iter()
            .filter(|name| seen.insert((*name).clone()))
            .cloned()
            .collect();
        let removal_set: HashSet<&str> = requested.iter().map(String::as_str).collect();

        let installed = requested
            .iter()
            .map(|name| {
                self.db
                    .get_installed(name)
                    .map(|keg| (name.clone(), keg.version))
                    .ok_or_else(|| Error::NotInstalled { name: name.clone() })
            })
            .collect::<Result<Vec<_>, _>>()?;

        if !force {
            for name in &requested {
                let dependents: Vec<String> = self
                    .db
                    .installed_dependents(name)?
                    .into_iter()
                    .filter(|dependent| !removal_set.contains(dependent.as_str()))
                    .collect();
                if !dependents.is_empty() {
                    return Err(Error::RequiredBy {
                        name: name.clone(),
                        dependents,
                    });
                }
            }
        }

        for (name, version) in installed {
            self.uninstall_by_version(&name, &version)?;
        }
        let autoremoved = self.autoremove()?;
        Ok(UninstallResult {
            requested,
            autoremoved,
        })
    }

    pub fn autoremove(&mut self) -> Result<Vec<String>, Error> {
        let mut removed = Vec::new();
        loop {
            let orphans = self.db.list_orphans()?;
            if orphans.is_empty() {
                break;
            }
            for orphan in orphans {
                self.uninstall_by_version(&orphan.name, &orphan.version)?;
                removed.push(orphan.name);
            }
        }
        Ok(removed)
    }

    pub fn uninstall_by_version(&mut self, name: &str, version: &str) -> Result<(), Error> {
        let keg_name = formula_token(name);

        let keg_path = self.cellar.keg_path(keg_name, version);
        self.linker.unlink_keg(&keg_path)?;

        {
            let tx = self.db.transaction()?;
            tx.record_uninstall(name)?;
            tx.commit()?;
        }

        self.cellar.remove_keg(keg_name, version)?;

        Ok(())
    }

    pub fn gc(&mut self) -> Result<Vec<String>, Error> {
        let unreferenced = self.db.get_unreferenced_store_keys()?;
        let mut removed = Vec::new();

        for store_key in unreferenced {
            self.store.remove_entry(&store_key)?;
            self.db.delete_store_ref(&store_key)?;
            removed.push(store_key);
        }

        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cellar::Cellar;
    use crate::installer::install::test_support::*;
    use crate::network::api::ApiClient;
    use crate::storage::blob::BlobCache;
    use crate::storage::db::Database;
    use crate::storage::store::Store;
    use crate::{Installer, Linker};

    fn installer_with_graph(tmp: &TempDir, edges: &[(&str, bool, &[&str])]) -> Installer {
        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();
        let api_client =
            ApiClient::with_base_url("http://127.0.0.1:9/formula".to_string()).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let mut db = Database::open(&root.join("db/zb.sqlite3")).unwrap();
        {
            let tx = db.transaction().unwrap();
            for (name, explicit, dependencies) in edges {
                let dependencies = dependencies
                    .iter()
                    .map(|dependency| (*dependency).to_string())
                    .collect::<Vec<_>>();
                tx.record_install(name, "1.0.0", name, *explicit, &dependencies)
                    .unwrap();
            }
            tx.commit().unwrap();
        }
        Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix,
            root.join("locks"),
        )
    }

    #[test]
    fn protected_uninstall_reports_installed_dependents() {
        let tmp = TempDir::new().unwrap();
        let mut installer =
            installer_with_graph(&tmp, &[("ffmpeg", true, &["x264"]), ("x264", false, &[])]);

        let err = installer
            .uninstall_many(&["x264".to_string()], false)
            .unwrap_err();
        assert!(matches!(
            err,
            zb_core::Error::RequiredBy { name, dependents }
                if name == "x264" && dependents == vec!["ffmpeg"]
        ));
        assert!(installer.is_installed("x264"));
    }

    #[test]
    fn removing_last_parent_recursively_removes_orphans() {
        let tmp = TempDir::new().unwrap();
        let mut installer = installer_with_graph(
            &tmp,
            &[
                ("ffmpeg", true, &["x264"]),
                ("x264", false, &["nasm"]),
                ("nasm", false, &[]),
            ],
        );

        let result = installer
            .uninstall_many(&["ffmpeg".to_string()], false)
            .unwrap();
        assert_eq!(result.requested, vec!["ffmpeg"]);
        assert_eq!(result.autoremoved, vec!["x264", "nasm"]);
        assert!(installer.list_installed().unwrap().is_empty());
    }

    #[test]
    fn force_removal_retains_incoming_declaration() {
        let tmp = TempDir::new().unwrap();
        let mut installer =
            installer_with_graph(&tmp, &[("ffmpeg", true, &["x264"]), ("x264", false, &[])]);

        installer
            .uninstall_many(&["x264".to_string()], true)
            .unwrap();
        assert_eq!(
            installer.db.installed_dependents("x264").unwrap(),
            vec!["ffmpeg"]
        );
    }

    #[test]
    fn shared_dependency_survives_until_final_parent_is_removed() {
        let tmp = TempDir::new().unwrap();
        let mut installer = installer_with_graph(
            &tmp,
            &[
                ("ffmpeg", true, &["x264"]),
                ("vlc", true, &["x264"]),
                ("x264", false, &[]),
            ],
        );

        let first = installer
            .uninstall_many(&["ffmpeg".to_string()], false)
            .unwrap();
        assert!(first.autoremoved.is_empty());
        assert!(installer.is_installed("x264"));

        let second = installer
            .uninstall_many(&["vlc".to_string()], false)
            .unwrap();
        assert_eq!(second.autoremoved, vec!["x264"]);
    }

    #[test]
    fn batch_removal_allows_edges_within_requested_set() {
        let tmp = TempDir::new().unwrap();
        let mut installer =
            installer_with_graph(&tmp, &[("ffmpeg", true, &["x264"]), ("x264", false, &[])]);

        let result = installer
            .uninstall_many(&["x264".to_string(), "ffmpeg".to_string()], false)
            .unwrap();
        assert_eq!(result.requested, vec!["x264", "ffmpeg"]);
        assert!(result.autoremoved.is_empty());
        assert!(installer.list_installed().unwrap().is_empty());
    }

    #[tokio::test]
    async fn uninstall_cleans_everything() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let bottle = create_bottle_tarball("uninstallme");
        let bottle_sha = sha256_hex(&bottle);

        let tag = get_test_bottle_tag();
        let formula_json = format!(
            r#"{{
                "name": "uninstallme",
                "versions": {{ "stable": "1.0.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/uninstallme-1.0.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            bottle_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/uninstallme.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&formula_json))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!(
                "/bottles/uninstallme-1.0.0.{}.bottle.tar.gz",
                tag
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bottle.clone()))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        installer
            .install(&["uninstallme".to_string()], true)
            .await
            .unwrap();

        assert!(installer.is_installed("uninstallme"));
        assert!(root.join("cellar/uninstallme/1.0.0").exists());
        assert!(prefix.join("bin/uninstallme").exists());

        installer.uninstall("uninstallme").unwrap();

        assert!(!installer.is_installed("uninstallme"));
        assert!(!root.join("cellar/uninstallme/1.0.0").exists());
        assert!(!prefix.join("bin/uninstallme").exists());
    }

    #[tokio::test]
    async fn gc_removes_unreferenced_store_entries() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let bottle = create_bottle_tarball("gctest");
        let bottle_sha = sha256_hex(&bottle);

        let tag = get_test_bottle_tag();
        let formula_json = format!(
            r#"{{
                "name": "gctest",
                "versions": {{ "stable": "1.0.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/gctest-1.0.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            bottle_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/gctest.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&formula_json))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/bottles/gctest-1.0.0.{}.bottle.tar.gz", tag)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bottle.clone()))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        installer
            .install(&["gctest".to_string()], true)
            .await
            .unwrap();

        assert!(root.join("store").join(&bottle_sha).exists());

        installer.uninstall("gctest").unwrap();

        assert!(root.join("store").join(&bottle_sha).exists());

        let removed = installer.gc().unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0], bottle_sha);

        assert!(!root.join("store").join(&bottle_sha).exists());
        assert!(
            installer
                .db
                .get_unreferenced_store_keys()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn gc_does_not_remove_referenced_store_entries() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let bottle = create_bottle_tarball("keepme");
        let bottle_sha = sha256_hex(&bottle);

        let tag = get_test_bottle_tag();
        let formula_json = format!(
            r#"{{
                "name": "keepme",
                "versions": {{ "stable": "1.0.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/keepme-1.0.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            bottle_sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/keepme.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&formula_json))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/bottles/keepme-1.0.0.{}.bottle.tar.gz", tag)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bottle.clone()))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.clone(),
            root.join("locks"),
        );

        installer
            .install(&["keepme".to_string()], true)
            .await
            .unwrap();

        assert!(root.join("store").join(&bottle_sha).exists());

        let removed = installer.gc().unwrap();
        assert!(removed.is_empty());

        assert!(root.join("store").join(&bottle_sha).exists());
    }

    #[tokio::test]
    async fn uninstall_accepts_full_tap_reference_after_install() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let bottle = create_bottle_tarball("terraform");
        let sha = sha256_hex(&bottle);
        let tag = get_test_bottle_tag();

        let tap_formula_rb = format!(
            r#"
class Terraform < Formula
  version "1.10.0"
  bottle do
    root_url "{}/v2/hashicorp/tap"
    sha256 {}: "{}"
  end
end
"#,
            mock_server.uri(),
            tag,
            sha
        );

        Mock::given(method("GET"))
            .and(path("/hashicorp/homebrew-tap/main/Formula/terraform.rb"))
            .respond_with(ResponseTemplate::new(200).set_body_string(tap_formula_rb))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!(
                "/v2/hashicorp/tap/terraform/blobs/sha256:{sha}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bottle))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client = ApiClient::with_base_url(format!("{}/formula", mock_server.uri()))
            .unwrap()
            .with_tap_raw_base_url(mock_server.uri());
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.to_path_buf(),
            root.join("locks"),
        );

        installer
            .install(&["hashicorp/tap/terraform".to_string()], true)
            .await
            .unwrap();

        assert!(installer.is_installed("hashicorp/tap/terraform"));
        assert!(!installer.is_installed("terraform"));
        assert!(root.join("cellar/terraform/1.10.0").exists());
        installer.uninstall("hashicorp/tap/terraform").unwrap();
        assert!(!installer.is_installed("hashicorp/tap/terraform"));
        assert!(!root.join("cellar/terraform/1.10.0").exists());
    }

    #[tokio::test]
    async fn uninstalling_non_installed_tap_ref_does_not_remove_core_formula() {
        let mock_server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();

        let bottle = create_bottle_tarball("terraform");
        let sha = sha256_hex(&bottle);
        let tag = get_test_bottle_tag();
        let core_json = format!(
            r#"{{
                "name": "terraform",
                "versions": {{ "stable": "1.10.0" }},
                "dependencies": [],
                "bottle": {{
                    "stable": {{
                        "files": {{
                            "{}": {{
                                "url": "{}/bottles/terraform-1.10.0.{}.bottle.tar.gz",
                                "sha256": "{}"
                            }}
                        }}
                    }}
                }}
            }}"#,
            tag,
            mock_server.uri(),
            tag,
            sha
        );

        Mock::given(method("GET"))
            .and(path("/formula/terraform.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(core_json))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!(
                "/bottles/terraform-1.10.0.{}.bottle.tar.gz",
                tag
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bottle))
            .mount(&mock_server)
            .await;

        let root = tmp.path().join("zerobrew");
        let prefix = tmp.path().join("homebrew");
        fs::create_dir_all(root.join("db")).unwrap();

        let api_client =
            ApiClient::with_base_url(format!("{}/formula", mock_server.uri())).unwrap();
        let blob_cache = BlobCache::new(&root.join("cache")).unwrap();
        let store = Store::new(&root).unwrap();
        let cellar = Cellar::new(&root).unwrap();
        let linker = Linker::new(&prefix).unwrap();
        let db = Database::open(&root.join("db/zb.sqlite3")).unwrap();

        let mut installer = Installer::new(
            api_client,
            blob_cache,
            store,
            cellar,
            linker,
            db,
            prefix.to_path_buf(),
            root.join("locks"),
        );
        installer
            .install(&["terraform".to_string()], true)
            .await
            .unwrap();
        assert!(installer.is_installed("terraform"));

        let err = installer.uninstall("hashicorp/tap/terraform").unwrap_err();
        assert!(matches!(err, zb_core::Error::NotInstalled { .. }));
        assert!(installer.is_installed("terraform"));
    }
}
