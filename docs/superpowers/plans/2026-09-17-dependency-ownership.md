# Dependency Ownership and Automatic Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist explicit/implicit formula ownership and direct dependency edges so uninstall is safe, upgrades discard obsolete dependencies, orphaned implicit formulas are removed, and `zb list` explains ownership.

**Architecture:** SQLite schema v2 stores an explicit bit on each installed formula plus normalized direct edges from dependent to dependency. Install and upgrade publish formula metadata through the existing per-formula transaction; uninstall validates reverse edges, removes requested formulas as a set, then invokes one shared recursive orphan sweeper. CLI commands are thin adapters over typed installer results.

**Tech Stack:** Rust 2024, rusqlite, clap, tokio, existing `zb_core`/`zb_io`/`zb_cli` workspace crates.

**Spec:** `docs/superpowers/specs/2026-09-17-dependency-ownership-design.md`

## Global Constraints

- Existing schema-v1 rows migrate as explicit; migration must never make an existing package removable.
- Dependency edges use normalized install names and represent direct runtime dependencies only.
- Existing explicit packages are never demoted; directly installing an implicit package promotes it.
- Casks remain explicit and have no dependency edges.
- Default uninstall refuses when an installed package outside the removal set depends on the target; `--force` bypasses that check.
- Forced removal retains incoming dependency declarations from still-installed parents.
- Cleanup removes only implicit formulas with zero installed direct dependents and repeats until stable.
- Automatic cleanup runs only after a fully successful uninstall or upgrade batch.
- No new third-party dependency is introduced.

---

### Task 1: Schema v2 and dependency ownership queries

**Files:**
- Modify: `zb_io/src/storage/db.rs`
- Modify: `zb_io/src/storage/mod.rs`
- Modify: `zb_io/src/lib.rs`

**Interfaces:**
- Produces: `InstalledKeg { name, version, store_key, installed_at, explicit }`.
- Produces: `InstalledFormula { keg: InstalledKeg, required_by: Vec<String> }`.
- Produces: `InstallTransaction::record_install(name, version, store_key, explicit, dependencies)`.
- Produces: `Database::{list_installed_with_ownership, installed_dependents, list_orphans}`.

- [ ] **Step 1: Add failing migration and query tests in `db.rs`**

Add tests that create a schema-v1 database manually, insert `ffmpeg`, open it through `Database::open`, and assert schema version 2 plus `explicit == true`. Add an in-memory graph test using `ffmpeg -> x264`, `vlc -> x264`, and `x264 -> nasm`:

```rust
#[test]
fn migration_v2_marks_existing_kegs_explicit() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(tmp.path()).unwrap();
    Database::migrate_to_v1(&conn).unwrap();
    Database::set_schema_version(&conn, 1).unwrap();
    conn.execute(
        "INSERT INTO installed_kegs VALUES ('ffmpeg', '7.1', 'ffmpeg-key', 1)",
        [],
    ).unwrap();
    drop(conn);

    let db = Database::open(tmp.path()).unwrap();
    assert_eq!(Database::get_schema_version(&db.conn).unwrap(), 2);
    assert!(db.get_installed("ffmpeg").unwrap().explicit);
}

#[test]
fn ownership_queries_handle_shared_and_recursive_dependencies() {
    let mut db = Database::in_memory().unwrap();
    let tx = db.transaction().unwrap();
    tx.record_install("ffmpeg", "7.1", "f", true, &["x264".into()]).unwrap();
    tx.record_install("vlc", "4.0", "v", true, &["x264".into()]).unwrap();
    tx.record_install("x264", "1", "x", false, &["nasm".into()]).unwrap();
    tx.record_install("nasm", "2", "n", false, &[]).unwrap();
    tx.commit().unwrap();

    assert_eq!(db.installed_dependents("x264").unwrap(), vec!["ffmpeg", "vlc"]);
    assert!(db.list_orphans().unwrap().is_empty());
}
```

Also test promotion/non-demotion, outgoing-edge replacement, cascading deletion of outgoing edges, retention of incoming edges, and alphabetical ordering of `required_by`.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
rtk cargo test -p zb_io storage::db::tests -- --nocapture
```

Expected: compilation fails because the new fields, schema version, methods, and expanded `record_install` signature do not exist.

- [ ] **Step 3: Implement schema v2 and typed query results**

In `db.rs`, enable foreign keys after each connection opens, increment `SCHEMA_VERSION`, and add the migration:

```rust
const SCHEMA_VERSION: u32 = 2;

fn configure_connection(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(Error::store("failed to configure database connection"))
}

fn migrate_to_v2(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "ALTER TABLE installed_kegs
             ADD COLUMN explicit INTEGER NOT NULL DEFAULT 1
             CHECK (explicit IN (0, 1));
         CREATE TABLE dependency_edges (
             dependent TEXT NOT NULL,
             dependency TEXT NOT NULL,
             PRIMARY KEY (dependent, dependency),
             FOREIGN KEY (dependent) REFERENCES installed_kegs(name) ON DELETE CASCADE
         );
         CREATE INDEX dependency_edges_dependency_idx
             ON dependency_edges(dependency);",
    ).map_err(Error::store("failed to migrate database to v2"))
}
```

Extend the model and transaction API:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledKeg {
    pub name: String,
    pub version: String,
    pub store_key: String,
    pub installed_at: i64,
    pub explicit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFormula {
    pub keg: InstalledKeg,
    pub required_by: Vec<String>,
}

pub fn record_install(
    &self,
    name: &str,
    version: &str,
    store_key: &str,
    explicit: bool,
    dependencies: &[String],
) -> Result<(), Error>
```

Use `explicit = installed_kegs.explicit OR excluded.explicit` in the upsert. After the upsert, delete `dependency_edges WHERE dependent = ?1`, then insert each unique dependency. Implement reverse-owner and orphan queries by joining the edge's `dependent` to `installed_kegs`; this deliberately ignores declarations from non-installed parents. Re-export `InstalledFormula` through `storage/mod.rs` and `zb_io/lib.rs`.

- [ ] **Step 4: Update existing storage test call sites to the new signature**

Every pre-existing test that is not testing dependency ownership should pass explicit ownership and no edges:

```rust
tx.record_install("foo", "1.0.0", "abc123", true, &[]).unwrap();
```

- [ ] **Step 5: Run storage tests and verify GREEN**

Run:

```bash
rtk cargo test -p zb_io storage::db::tests -- --nocapture
```

Expected: all storage database tests pass.

- [ ] **Step 6: Commit Task 1**

```bash
rtk git add zb_io/src/storage/db.rs zb_io/src/storage/mod.rs zb_io/src/lib.rs
rtk git commit -m "feat: persist formula dependency ownership"
```

---

### Task 2: Carry ownership through install plans and package transactions

**Files:**
- Modify: `zb_io/src/installer/install/mod.rs`
- Modify: `zb_io/src/installer/install/plan.rs`
- Modify: `zb_io/src/installer/install/bottle.rs`
- Modify: `zb_io/src/installer/install/source.rs`
- Modify: `zb_io/src/installer/install/outdated.rs`

**Interfaces:**
- Consumes: Task 1's expanded `record_install`.
- Produces: `PlannedInstall { install_name, formula, method, explicit }`.
- Produces: `InstallPlan { items }` where requested roots have `explicit = true` and closure-only items have `explicit = false`.

- [ ] **Step 1: Add failing planning tests**

In `plan.rs`, extend existing wiremock tests with a root `ffmpeg` depending on `x264`; assert ownership flags:

```rust
let plan = installer.plan(&["ffmpeg".to_string()]).await.unwrap();
let ownership: BTreeMap<_, _> = plan.items.iter()
    .map(|item| (item.install_name.as_str(), item.explicit))
    .collect();
assert_eq!(ownership["ffmpeg"], true);
assert_eq!(ownership["x264"], false);
```

Add an installer test that first installs `x264` implicitly, calls a direct `plan`/`execute` for `x264`, and asserts `get_installed("x264").explicit` becomes true.

- [ ] **Step 2: Run focused install tests and verify RED**

Run:

```bash
rtk cargo test -p zb_io installer::install::plan::tests -- --nocapture
```

Expected: compilation fails because `PlannedInstall::explicit` does not exist and installation does not provide ownership metadata.

- [ ] **Step 3: Mark requested roots in the plan**

Build a root set before converting ordered names to items:

```rust
let explicit_roots: HashSet<&str> = names.iter().map(String::as_str).collect();
for install_name in ordered {
    let explicit = explicit_roots.contains(install_name.as_str());
    let formula = formulas.get(&install_name).cloned().unwrap();
    items.push(self.plan_item(install_name, formula, build_from_source, explicit)?);
}
```

Add `pub explicit: bool` to `PlannedInstall` and thread the value through both normal and best-effort planning.

- [ ] **Step 4: Persist direct runtime edges in bottle and source transactions**

At both `record_install` call sites, take the runtime dependency snapshot before opening the transaction and pass the planned ownership:

```rust
let dependencies = item.formula.runtime_dependencies();
tx.record_install(
    install_name,
    &version,
    store_key,
    item.explicit,
    &dependencies,
)?;
```

For casks, pass `true, &[]`. Update direct database setup in `outdated.rs` tests to pass `true, &[]`.

- [ ] **Step 5: Run install tests and verify GREEN**

Run:

```bash
rtk cargo test -p zb_io installer::install -- --nocapture
```

Expected: install tests pass, including root/dependency ownership and promotion.

- [ ] **Step 6: Commit Task 2**

```bash
rtk git add zb_io/src/installer/install zb_io/src/storage/db.rs
rtk git commit -m "feat: record ownership during formula install"
```

---

### Task 3: Protected batch uninstall and recursive autoremove

**Files:**
- Modify: `zb_core/src/errors.rs`
- Modify: `zb_io/src/installer/install/mod.rs`
- Modify: `zb_io/src/installer/install/uninstall.rs`

**Interfaces:**
- Produces: `Error::RequiredBy { name: String, dependents: Vec<String> }`.
- Produces: `UninstallResult { requested: Vec<String>, autoremoved: Vec<String> }`.
- Produces: `Installer::uninstall_many(&[String], force: bool) -> Result<UninstallResult, Error>`.
- Produces: `Installer::autoremove() -> Result<Vec<String>, Error>`.
- Retains: `Installer::uninstall(name)` as a compatibility wrapper using `force = false`.

- [ ] **Step 1: Add failing error-display and installer behavior tests**

In `errors.rs`:

```rust
#[test]
fn required_by_error_names_sorted_dependents_and_force_escape_hatch() {
    let err = Error::RequiredBy {
        name: "x264".into(),
        dependents: vec!["ffmpeg".into(), "vlc".into()],
    };
    assert_eq!(
        err.to_string(),
        "cannot uninstall 'x264'; required by: ffmpeg, vlc (use --force to remove anyway)"
    );
}
```

In `uninstall.rs`, construct package graphs with the existing test installer and assert:

```rust
let err = installer.uninstall_many(&["x264".into()], false).unwrap_err();
assert!(matches!(err, Error::RequiredBy { .. }));

let result = installer.uninstall_many(&["ffmpeg".into()], false).unwrap();
assert_eq!(result.requested, vec!["ffmpeg"]);
assert_eq!(result.autoremoved, vec!["nasm", "x264"]);
```

Add separate cases for shared dependencies, forced removal retaining `ffmpeg -> x264`, removal-set internal edges, `--all`-equivalent full sets, and explicit dependencies surviving a sweep.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
rtk cargo test -p zb_core errors::tests::required_by -- --nocapture
rtk cargo test -p zb_io installer::install::uninstall::tests -- --nocapture
```

Expected: compilation fails because the error, result type, batch API, and sweeper do not exist.

- [ ] **Step 3: Add the protected-uninstall error**

Add the variant and display arm:

```rust
RequiredBy { name: String, dependents: Vec<String> },

Error::RequiredBy { name, dependents } => write!(
    f,
    "cannot uninstall '{name}'; required by: {} (use --force to remove anyway)",
    dependents.join(", ")
),
```

- [ ] **Step 4: Implement preflight validation and batch removal**

Before touching files, collect requested names into a `HashSet`. Unless forced, call `installed_dependents` for every target, filter out names in the removal set, and return `RequiredBy` on the first blocked target. Only after every target passes should removal begin.

```rust
pub struct UninstallResult {
    pub requested: Vec<String>,
    pub autoremoved: Vec<String>,
}

pub fn uninstall_many(
    &mut self,
    names: &[String],
    force: bool,
) -> Result<UninstallResult, Error>
```

Deduplicate targets while preserving argument order. Keep `uninstall_by_version` as the low-level unlink/database/cellar primitive.

- [ ] **Step 5: Implement the shared recursive orphan sweep**

Use stable database ordering and iterate until no candidates remain:

```rust
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
```

Call it once after all requested removals succeed. The compatibility `uninstall(name)` delegates to `uninstall_many(&[name.to_owned()], false)`.

- [ ] **Step 6: Run core and uninstall tests and verify GREEN**

Run:

```bash
rtk cargo test -p zb_core errors::tests -- --nocapture
rtk cargo test -p zb_io installer::install::uninstall::tests -- --nocapture
```

Expected: protected, forced, shared, recursive, and batch uninstall tests pass.

- [ ] **Step 7: Commit Task 3**

```bash
rtk git add zb_core/src/errors.rs zb_io/src/installer/install/mod.rs zb_io/src/installer/install/uninstall.rs
rtk git commit -m "feat: protect dependencies and autoremove orphans"
```

---

### Task 4: Reconcile dependency ownership safely during upgrades

**Files:**
- Modify: `zb_io/src/installer/install/mod.rs`
- Modify: `zb_io/src/installer/install/upgrade.rs`
- Modify: `zb_cli/src/commands/upgrade.rs`

**Interfaces:**
- Consumes: `PlannedInstall::explicit`, `Installer::autoremove`.
- Produces: `Installer::remove_keg_artifacts(name, version)` that does not mutate ownership rows.
- Produces: upgrade behavior that preserves the old explicit bit and replaces edges only when the new package record commits.

- [ ] **Step 1: Add failing upgrade ownership tests**

Extend `upgrade.rs` wiremock fixtures so v1 of `app` depends on `olddep`, v2 depends on `newdep`. Assert after upgrade:

```rust
let app = installer.get_installed("app").unwrap();
assert!(app.explicit);
assert_eq!(installer.db.installed_dependents("olddep").unwrap(), Vec::<String>::new());
assert_eq!(installer.db.installed_dependents("newdep").unwrap(), vec!["app"]);
let removed = installer.autoremove().unwrap();
assert_eq!(removed, vec!["olddep"]);
assert!(!installer.is_installed("olddep"));
assert!(installer.is_installed("newdep"));
```

Add a second test that installs `app` as a dependency of `suite`, upgrades `app`, and asserts it remains implicit. Add a failed-upgrade test that asserts no orphan sweep occurs.

- [ ] **Step 2: Run upgrade tests and verify RED**

Run:

```bash
rtk cargo test -p zb_io installer::install::upgrade::tests -- --nocapture
```

Expected: assertions fail because upgrade currently deletes ownership before reinstall and never removes dropped dependencies.

- [ ] **Step 3: Separate artifact removal from database removal**

Extract the physical portion of uninstall:

```rust
fn remove_keg_artifacts(&mut self, name: &str, version: &str) -> Result<(), Error> {
    let keg_name = formula_token(name);
    let keg_path = self.cellar.keg_path(keg_name, version);
    self.linker.unlink_keg(&keg_path)?;
    self.cellar.remove_keg(keg_name, version)
}
```

Normal uninstall calls this together with `record_uninstall`. Upgrade uses it while retaining the old database row and edge snapshot until the replacement item's `record_install` transaction commits. If replacement fails after artifact removal, delete the stale installed row without invoking orphan cleanup and return the original upgrade error.

- [ ] **Step 4: Preserve the root's ownership in the upgrade plan**

After planning and before execution, override only the matching root item:

```rust
let was_explicit = old.explicit;
let mut plan = self.plan_with_options(&[name.to_string()], build_from_source).await?;
if let Some(root) = plan.items.iter_mut().find(|item| item.install_name == name) {
    root.explicit = was_explicit;
}
```

Dependencies retain `explicit = false`, and Task 1's upsert preserves any dependency that was already explicit.

- [ ] **Step 5: Sweep once after a successful CLI upgrade batch**

In `zb_cli/src/commands/upgrade.rs`, call `installer.autoremove()` only when both `errors` and `missing` are empty, after all package upgrades finish. Include the removed count in the final output when nonzero:

```rust
let autoremoved = installer.autoremove()?;
if !autoremoved.is_empty() {
    ui.info(format!("Removed unused dependencies: {}", autoremoved.join(", ")))
        .map_err(ui_error)?;
}
```

- [ ] **Step 6: Run upgrade tests and verify GREEN**

Run:

```bash
rtk cargo test -p zb_io installer::install::upgrade::tests -- --nocapture
rtk cargo test -p zb_cli commands::upgrade -- --nocapture
```

Expected: explicit/implicit preservation, edge replacement, dropped-dependency cleanup, and failed-batch behavior pass.

- [ ] **Step 7: Commit Task 4**

```bash
rtk git add zb_io/src/installer/install zb_cli/src/commands/upgrade.rs
rtk git commit -m "feat: reconcile dependencies on upgrade"
```

---

### Task 5: Expose force, autoremove, and ownership-aware list output

**Files:**
- Create: `zb_cli/src/commands/autoremove.rs`
- Modify: `zb_cli/src/commands/mod.rs`
- Modify: `zb_cli/src/commands/uninstall.rs`
- Modify: `zb_cli/src/commands/list.rs`
- Modify: `zb_cli/src/cli.rs`
- Modify: `zb_cli/src/bin/zb.rs`

**Interfaces:**
- Consumes: `Installer::{uninstall_many, autoremove, list_installed_with_ownership}`.
- Produces: `Commands::Autoremove` and `Commands::Uninstall { force }`.
- Produces: stable human-readable ownership labels.

- [ ] **Step 1: Add failing clap parser tests**

In `cli.rs`:

```rust
#[test]
fn uninstall_accepts_force() {
    let cli = Cli::try_parse_from(["zb", "uninstall", "x264", "--force"]).unwrap();
    assert!(matches!(cli.command, Commands::Uninstall { force: true, .. }));
}

#[test]
fn accepts_autoremove_command() {
    let cli = Cli::try_parse_from(["zb", "autoremove"]).unwrap();
    assert!(matches!(cli.command, Commands::Autoremove));
}
```

Add pure formatting tests in `list.rs` for explicit, implicit required, explicit also-required, and implicit orphan labels.

- [ ] **Step 2: Run CLI unit tests and verify RED**

Run:

```bash
rtk cargo test -p zb_cli cli::tests -- --nocapture
rtk cargo test -p zb_cli commands::list -- --nocapture
```

Expected: parser compilation fails because the new flag and command are absent; list formatting tests fail because ownership is not rendered.

- [ ] **Step 3: Add CLI definitions and dispatch**

Extend the command enum:

```rust
Uninstall {
    #[arg(required_unless_present = "all", num_args = 1..)]
    formulas: Vec<String>,
    #[arg(long, help = "Uninstall even when installed formulas depend on the target")]
    force: bool,
    #[arg(long, help = "Uninstall all installed packages")]
    all: bool,
},
/// Remove installed dependencies that are no longer required
Autoremove,
```

Export `commands::autoremove` and add both dispatch arms in `bin/zb.rs`.

- [ ] **Step 4: Convert uninstall command handling to one batch call**

Normalize all names, derive the full installed-name set for `--all`, and invoke exactly once:

```rust
let result = installer.uninstall_many(&formulas, force || all)?;
for name in &result.autoremoved {
    ui.info(format!("Removed unused dependency {name}"))
        .map_err(ui_error)?;
}
```

This prevents sequential cleanup from removing a later requested target and ensures blockers are checked before mutation.

- [ ] **Step 5: Implement autoremove command and ownership formatting**

`autoremove.rs` reports either `No unused dependencies.` or a stable joined list. In `list.rs`, isolate label construction for unit testing:

```rust
fn ownership_label(explicit: bool, required_by: &[String]) -> String {
    match (explicit, required_by.is_empty()) {
        (true, true) => "explicit".into(),
        (true, false) => format!("explicit (also required by {})", required_by.join(", ")),
        (false, false) => format!("implicit (required by {})", required_by.join(", ")),
        (false, true) => "implicit (orphan)".into(),
    }
}
```

Render `name`, `version`, and this label from `list_installed_with_ownership()`.

- [ ] **Step 6: Run CLI tests and verify GREEN**

Run:

```bash
rtk cargo test -p zb_cli --lib --bins -- --nocapture
```

Expected: parser, command dispatch, uninstall, autoremove, and list-format tests pass.

- [ ] **Step 7: Commit Task 5**

```bash
rtk git add zb_cli/src
rtk git commit -m "feat: expose dependency ownership commands"
```

---

### Task 6: Cross-layer regression tests, documentation, and final verification

**Files:**
- Modify: `zb_cli/tests/integration.rs`
- Modify: `README.md`
- Modify: `README.zh.md`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: all prior task interfaces.
- Produces: user documentation and end-to-end regression coverage.

- [ ] **Step 1: Add ignored end-to-end dependency lifecycle coverage**

Add a network-backed ignored test using a stable formula with dependencies, asserting install/list/protected uninstall/force or parent-uninstall/autoremove behavior through the binary:

```rust
#[test]
#[ignore = "integration test"]
fn test_dependency_ownership_lifecycle() {
    let t = TestEnv::new();
    assert_success(&t.zb(&["install", "ffmpeg"]), "install ffmpeg");

    let listed = t.zb(&["list"]);
    assert_success(&listed, "list ownership");
    assert_stdout_contains(&listed, "ffmpeg");
    assert_stdout_contains(&listed, "explicit");
    assert_stdout_contains(&listed, "implicit (required by");
}
```

Keep deterministic shared/drop-on-upgrade assertions in the wiremock unit tests from Tasks 2–4; the ignored test only validates the real CLI wiring.

- [ ] **Step 2: Run the new integration test and confirm its behavior**

Run only when network/platform prerequisites are available:

```bash
rtk cargo test -p zb_cli --test integration test_dependency_ownership_lifecycle -- --ignored --nocapture
```

Expected: PASS. If the upstream formula is unavailable, record that environmental limitation and rely on the deterministic wiremock coverage; do not weaken assertions.

- [ ] **Step 3: Document user-facing behavior**

Add concise command examples to both READMEs:

```text
zb list                       # show explicit installs and dependency ownership
zb uninstall x264             # refuses while another formula requires it
zb uninstall x264 --force     # remove despite installed dependents
zb autoremove                 # remove unused implicit dependencies
```

Add an Unreleased changelog entry covering persistent ownership, safe uninstall, automatic cleanup after uninstall/upgrade, and `autoremove`.

- [ ] **Step 4: Run formatting and the full workspace test suite**

Run:

```bash
rtk cargo fmt --check
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Expected: every command exits 0 with no warnings.

- [ ] **Step 5: Inspect the final diff for schema and CLI completeness**

Run:

```bash
rtk git diff --check
rtk git status --short
rtk git diff --stat
```

Expected: no whitespace errors; only dependency-ownership implementation, tests, and documentation are changed.

- [ ] **Step 6: Commit Task 6**

```bash
rtk git add README.md README.zh.md CHANGELOG.md zb_cli/tests/integration.rs
rtk git commit -m "docs: explain dependency ownership cleanup"
```
