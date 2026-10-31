; Inno Setup script. Build with:  iscc /DAppVersion=0.1.0 packaging\windows\agentory.iss
; Expects the staged folder from packaging/stage.sh at dist\Agentory.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{7D3B6A52-9C1E-4F0B-8A47-5E2C91D0B3A6}
AppName=Agentory
AppVersion={#AppVersion}
AppPublisher=AminPerez
DefaultDirName={autopf}\Agentory
DefaultGroupName=Agentory
UninstallDisplayName=Agentory {#AppVersion}
UninstallDisplayIcon={app}\agentory.exe
SetupIconFile=..\icon.ico
OutputDir=..\..\dist
OutputBaseFilename=Agentory-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
; Per-user install by default; the dialog lets users pick.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
DisableProgramGroupPage=yes
LicenseFile=..\..\dist\Agentory\NOTICE.txt

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"

[Files]
Source: "..\..\dist\Agentory\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion
; Exe layout the helper manages (see docs/self_built_phase1_helper.md). A reinstall resets `active`
; to the shipped build, except when self-build is enabled (marker file): then `active` stays the
; user's last good self-build and upstream changes only arrive through the in-app merge flow.
; `fallback` is the last resort and always follows the official release.
Source: "..\..\dist\Agentory\agentory.exe"; DestDir: "{userappdata}\Agentory\bin"; DestName: "active.exe"; Flags: ignoreversion; Check: ShouldReplaceActive
Source: "..\..\dist\Agentory\agentory.exe"; DestDir: "{userappdata}\Agentory\bin"; DestName: "fallback.exe"; Flags: ignoreversion

[Icons]
Name: "{group}\Agentory"; Filename: "{app}\agentory-helper.exe"; Parameters: "launch"; IconFilename: "{app}\agentory.exe"
Name: "{autodesktop}\Agentory"; Filename: "{app}\agentory-helper.exe"; Parameters: "launch"; IconFilename: "{app}\agentory.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\agentory-helper.exe"; Parameters: "launch"; Description: "Launch Agentory"; Flags: nowait postinstall skipifsilent

[Code]
// Keep the user's self-built exe: only replace `active.exe` when self-build is off (no marker, see
// `Paths::selfbuild_marker`) or when there is no active exe to keep.
function ShouldReplaceActive: Boolean;
begin
  Result := not FileExists(ExpandConstant('{userappdata}\Agentory\selfbuild'))
    or not FileExists(ExpandConstant('{userappdata}\Agentory\bin\active.exe'));
end;

// Offer to delete the app data (project list, settings, logs, session worktrees). Project folders
// are never touched. Skipped for silent uninstalls and for updates.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    if MsgBox('Do you also want to delete all Agentory application data (project list, settings, logs and session worktrees)?' + #13#10 + #13#10 + 'Your project folders will not be deleted.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    begin
      DelTree(ExpandConstant('{userappdata}\Agentory'), True, True, True);
      DelTree(ExpandConstant('{localappdata}\Agentory'), True, True, True);
    end;
end;
