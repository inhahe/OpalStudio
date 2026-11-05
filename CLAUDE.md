# Opal Studio

- **Architecture and features:** `design.md`; what is built and what comes next:
  `roadmap.md`; open problems: `known-issues.md`. Read `design.md` before a change whose
  effects leave the file you are editing.
- **User-facing document:** `README.md` (see *User-Facing Documentation* in the global
  rules).
- **The exe in this folder is always the latest release build.** After any change to the
  application's code is committed, run `build-exe.bat` (from the PowerShell tool:
  `& .\build-exe.bat`, in the background, since a release build takes minutes), before
  reporting the work as done. (It is not `release.bat`: in the user's other projects that
  name publishes a GitHub release.) It is part of the user's own workflow, not something
  to document in `README.md`. It builds the release version and copies `opal-studio.exe`
  here; if the user has that copy open, the running one is renamed to
  `opal-studio.old.exe` and the new one put in its place. Both are kept out of git.
