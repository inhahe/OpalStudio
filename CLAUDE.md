# Opal Studio

- **Architecture and features:** `design.md`; what is built and what comes next:
  `roadmap.md`; open problems: `known-issues.md`. Read `design.md` before a change whose
  effects leave the file you are editing.
- **User-facing document:** `README.md` (see *User-Facing Documentation* in the global
  rules).
- **The exe in this folder is always the latest release build.** After any change to the
  application's code is committed, run `build-exe.bat` (from the PowerShell tool:
  `& .\build-exe.bat`, in the background, since a release build takes minutes), before
  reporting the work as done. It is part of the user's own workflow, not something to
  document in `README.md`. It builds the release version and copies `opal-studio.exe` here;
  if the user has that copy open, the running one is renamed to `opal-studio.old.exe` and
  the new one put in its place. Both are kept out of git.
- **`release.bat` publishes a GitHub release** (github.com/inhahe/OpalStudio), only when the
  user asks for one: the commit checked out, tagged `v<version>`, with a zip of the release
  build, `opal-studio.com`, `README.md`, the licences and the `python` package. It refuses if
  that version is released already, if anything tracked is not committed, if the commit is
  not pushed, or if the two version numbers below differ. Run it from the PowerShell tool
  (`& .\release.bat`), in the background: it builds first.
- **Bump the version whenever something is fixed or added**, in the commit that does it:
  `version` in `Cargo.toml`'s `[workspace.package]` (the one the exe reports) and in
  `python/pyproject.toml`, always the same. Bump the **patch** for bug fixes, refactors and
  plain rebuilds; the **minor** for new features or noticeable changes to existing ones; the
  **major** for breaking changes, or for a milestone overhaul (a redesigned interface, a
  substantially reworked set of features) even when nothing is actually incompatible.
  Changes to documentation or tests alone change nothing released, and bump nothing.
