# Agentory

A desktop app that hosts agent CLI sessions in terminals. Developers can also have it modify itself through an agent.

## Language

### Self-building

**Self-build**:
The opt-in mode in which a user changes Agentory itself and runs the result. Off by default.
_Avoid_: Self-modify, auto-update

**Modify session**:
A session in which an agent CLI edits the user's source clone inside a worktree on behalf of the user.
_Avoid_: Change request, patch session

**Source clone**:
The user's local git clone of the public repo, checked out at the installed version's tag, holding their local branch.
_Avoid_: Source dir, fork

**Gate**:
The automated check a build must pass before it can be installed: tests plus a smoke launch.
_Avoid_: CI, validation

**Smoke launch**:
Starting a built exe and requiring it to stay alive for 10 seconds and write its ready marker.
_Avoid_: Health check

**Ready marker**:
The file `%APPDATA%\Agentory\ready`, holding the PID and build id (git sha), that a running Agentory writes after its first frame renders, proving the smoke launch succeeded.

**User branch**:
The local branch in the source clone that holds the user's applied changes and receives upstream merges.
_Avoid_: Fork, custom branch

**Apply**:
The user's approval of a Modify session's diff, which triggers merge, build, gate and install.
_Avoid_: Accept, deploy

**Helper**:
The separate installed binary that swaps the exe, relaunches Agentory and reverts on a failed start. It is not built from the source clone.
_Avoid_: Updater, launcher

**Protected core**:
The helper and anything else that recovery depends on, which agents cannot change.

**Known-good exe**:
The last exe that passed the gate and started successfully, kept so the helper can revert to it.
_Avoid_: Backup, previous version

**Upstream merge**:
Bringing a new official release tag into the user's local branch.
_Avoid_: Update, rebase
