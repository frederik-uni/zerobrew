# Dependency Ownership and Automatic Cleanup Design

## Summary

Zerobrew will persist why each formula is installed. A formula is either explicitly installed by the user or implicitly installed as a dependency, and the database stores every direct `dependent -> dependency` relationship from the formula metadata used for the successful install. This enables dependency-safe uninstall, automatic recursive orphan removal after uninstall and upgrade, a manual `zb autoremove` command, and informative `zb list` output.

## Goals

- Preserve direct dependency relationships after installation.
- Distinguish explicitly requested formulas from implicitly installed dependencies.
- Allow any number of installed formulas to depend on the same installed dependency.
- Refuse removal of a formula that installed formulas still require unless `--force` is supplied.
- Automatically remove implicit dependencies once no installed formula requires them.
- Remove dependencies dropped by formula upgrades.
- Explain install ownership in `zb list`.
- Migrate existing installations without risking unexpected removal.

## Non-goals

- Dependency tracking for casks. Casks remain explicit installs with no dependency edges.
- Displaying every transitive root that ultimately caused a dependency to be installed. Output shows authoritative direct dependents only.
- Automatically repairing a dependency deliberately removed with `--force`.
- Changing Homebrew formula dependency semantics or resolving multiple installed versions of one formula. Installed formula identity remains its normalized install name.

## Persistent Data Model

The database schema advances from version 1 to version 2.

`installed_kegs` gains:

```sql
explicit INTEGER NOT NULL DEFAULT 1 CHECK (explicit IN (0, 1))
```

Every row already present when migration runs receives `explicit = 1`. Zerobrew cannot reconstruct historical ownership safely, so this conservative default ensures migration never makes an existing formula eligible for automatic removal.

A new table stores direct formula relationships:

```sql
CREATE TABLE dependency_edges (
    dependent TEXT NOT NULL,
    dependency TEXT NOT NULL,
    PRIMARY KEY (dependent, dependency),
    FOREIGN KEY (dependent) REFERENCES installed_kegs(name) ON DELETE CASCADE
);
CREATE INDEX dependency_edges_dependency_idx
    ON dependency_edges(dependency);
```

Only `dependent` has a foreign key. A forced uninstall may intentionally remove the target of an edge while its dependent remains installed. Retaining that edge records the dependent's still-unsatisfied declaration and allows a later reinstall of the missing formula to restore a consistent installation without reconstructing metadata.

SQLite foreign-key enforcement will be enabled for every database connection so deleting a dependent removes its outgoing edges. Incoming edges are retained when a dependency target is removed.

## Installation Semantics

An install plan carries the set of user-requested formula roots in addition to its dependency-ordered items.

For `zb install ffmpeg`:

- `ffmpeg` is recorded as explicit.
- Other formulas in the resolved closure are recorded as implicit unless they were already explicit.
- Directly installing an already-installed implicit formula promotes it to explicit, even if its package artifacts do not otherwise need replacement.
- An existing explicit formula is never demoted by appearing as another formula's dependency.
- Each successfully installed formula replaces its complete set of outgoing dependency edges with the runtime dependencies in the formula metadata used for that install.

The installed-keg update, explicit-state merge, and outgoing-edge replacement occur in one SQLite transaction for each successfully materialized formula. A failed formula installation does not publish its new dependency snapshot.

If an installation fails after some dependencies were installed successfully, those successful rows remain accurate implicit installs. The command does not run automatic orphan cleanup after failure; a later successful operation or `zb autoremove` can remove any zero-owner remnants.

Casks are recorded as explicit and have no dependency edges.

## Uninstall Semantics

By default, uninstall queries incoming edges from installed dependents. If any installed formula outside the requested removal set depends on a target, Zerobrew refuses removal and reports the direct dependents.

`zb uninstall <formula> --force` bypasses this check. Incoming edges remain in the database because the still-installed parents continue to declare that dependency. The forced removal may therefore leave those parents operationally broken, which is the explicit meaning of the flag.

Multi-formula uninstall is evaluated as a set. Edges originating from another formula in that same requested set do not block removal. `zb uninstall --all` removes the complete installed set without requiring `--force`.

After every fully successful uninstall command, Zerobrew runs the orphan sweep described below. It does not sweep after a partially failed uninstall command.

## Upgrade Semantics

Upgrade preserves the upgraded formula's existing explicit state. Dependencies installed as part of the new plan remain implicit unless already explicit, and each upgraded or installed formula replaces its outgoing edge snapshot with current metadata.

The old dependency snapshot remains authoritative until the replacement formula is successfully installed and its new edges are committed. A failed upgrade batch does not run orphan cleanup.

After a successful upgrade batch, Zerobrew runs one orphan sweep. Thus, a dependency removed from a formula's new metadata is deleted only when it is implicit and no other installed formula still references it. Running one sweep after the batch also prevents cleanup from deleting a package that a later item in the same upgrade batch still needs.

## Orphan Cleanup

An orphan is an installed formula for which:

- `explicit = 0`, and
- no edge from another currently installed formula targets it.

Cleanup repeatedly finds all current orphans, removes them using the existing unlink/database/cellar removal behavior, deletes their outgoing edges, and repeats until no orphan remains. Repetition is required because removing one implicit formula can orphan its own dependencies.

Shared dependencies remain installed until their last installed dependent is removed or drops the edge. Explicit formulas are never removed by orphan cleanup, even when they have no dependents.

`zb autoremove` runs this same sweep on demand and reports each removed formula. It is useful after older partial installs or interrupted workflows; immediately after migration it removes nothing because all migrated rows are explicit.

## CLI Behavior

`zb uninstall` gains `--force`:

```text
zb uninstall x264
error: cannot uninstall 'x264'; required by: ffmpeg

zb uninstall x264 --force
```

A new command triggers manual cleanup:

```text
zb autoremove
```

`zb list` shows the version, ownership state, and direct installed dependents in stable alphabetical order:

```text
ffmpeg   7.1    explicit
x264     r3108  implicit (required by ffmpeg)
openssl@3 3.5   explicit (also required by ffmpeg, wget)
```

The labels are:

- `explicit` when directly requested or conservatively migrated.
- `implicit (required by A, B)` for an implicit formula with installed direct dependents.
- `explicit (also required by A, B)` when an explicit formula is also a dependency.
- `implicit (orphan)` only if such a row is visible before a manual or automatic sweep; this makes interrupted-state diagnosis clear.

## Errors and Recovery

A dedicated core error variant carries the target formula and sorted direct dependents for protected uninstall. CLI rendering tells the user that `--force` is available.

Database mutations use transactions. Filesystem unlinking and cellar deletion continue to follow the existing installer ordering and error model; this feature does not attempt a cross-filesystem/SQLite distributed transaction.

Dependency edges store normalized install names matching `installed_kegs.name`. Duplicate formula arguments and shared dependencies produce only one edge because of the composite primary key.

## Testing Strategy

Storage tests will verify:

- v1-to-v2 migration marks existing rows explicit.
- explicit state cannot be demoted and can be promoted.
- outgoing edges are atomically replaced.
- reverse-dependent queries return stable, installed dependents.
- deleting a dependent removes its outgoing edges while forced deletion of a dependency retains incoming edges.
- orphan discovery handles shared and recursive dependency structures.

Installer tests will verify:

- roots are explicit and resolved dependencies implicit.
- directly installing an implicit formula promotes it.
- uninstall refuses when dependents exist and reports them.
- forced uninstall succeeds while retaining declarations from installed parents.
- removing the last parent recursively removes implicit dependencies.
- shared dependencies survive until the final parent is removed.
- upgrade drops obsolete edges and removes newly orphaned dependencies.
- upgrade preserves explicit state.
- failed upgrade does not sweep based on partial state.
- multi-target and `--all` uninstall treat requested formulas as one set.

CLI tests will verify `--force` parsing, `autoremove` dispatch, protected-uninstall messaging, autoremove reporting, and the four list-output states.

The full Rust workspace test suite and formatting checks must pass.
