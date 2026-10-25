; The Windows installer (docs/design/release.md §4), compiled by Inno Setup 6
; from package.ps1, which defines Version, Stage (the folder with the files)
; and Output. Saved as UTF-8 with a byte order mark: Inno Setup reads the
; Chinese messages right only so.

#ifndef Version
  #error Run package.ps1, which defines Version, Stage and Output.
#endif

[Setup]
; Never changes: Windows tells an upgrade from a second application by it.
AppId={{F1BB9C74-A6D3-4098-8533-050A889A1539}
AppName=Materials Workbench
AppVersion={#Version}
AppVerName=Materials Workbench {#Version}
AppPublisher=Materials Workbench
AppPublisherURL=https://github.com/Ember-Moth/Materials-Workbench
AppSupportURL=https://github.com/Ember-Moth/Materials-Workbench/issues
AppUpdatesURL=https://github.com/Ember-Moth/Materials-Workbench/releases
VersionInfoVersion={#Version}
; For the current user, without administrator rights, unless the user picks
; all users: %LOCALAPPDATA%\Programs\Materials Workbench.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
DefaultDirName={autopf}\Materials Workbench
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
ChangesEnvironment=yes
ChangesAssociations=yes
SetupIconFile=..\icons\materials-workbench.ico
UninstallDisplayIcon={app}\materials-workbench.exe
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir={#Output}
OutputBaseFilename=Materials-Workbench-{#Version}-windows-x64-setup
; Signing (not set up, §4 and §10): the executables are signed before they
; come here; the installer and its uninstaller with
;   SignTool=… and SignedUninstaller=yes

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
#if FileExists(AddBackslash(CompilerPath) + "Languages\ChineseSimplified.isl")
  #define Chinese
Name: "zh_CN"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
#endif

[CustomMessages]
en.AddToPath=Add the command line tool mw to PATH
en.Associate=Open .cif and .vasp files with Materials Workbench ("Open with")
en.CifFile=Crystallographic Information File
en.PoscarFile=VASP POSCAR
#ifdef Chinese
zh_CN.AddToPath=把命令行工具 mw 加进 PATH
zh_CN.Associate=用 Materials Workbench 打开 .cif 和 .vasp 文件（"打开方式"）
zh_CN.CifFile=晶体学信息文件（CIF）
zh_CN.PoscarFile=VASP POSCAR
#endif

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "{cm:AddToPath}"; Flags: unchecked
Name: "associate"; Description: "{cm:Associate}"; Flags: unchecked

[Files]
Source: "{#Stage}\materials-workbench.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\mw.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD-PARTY-LICENSES.html"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; The application sets the same AppUserModelID (apps/workbench/src/launch.rs),
; so that the taskbar groups its window with these shortcuts.
Name: "{autoprograms}\Materials Workbench"; Filename: "{app}\materials-workbench.exe"; AppUserModelID: "io.github.ember-moth.MaterialsWorkbench"
Name: "{autodesktop}\Materials Workbench"; Filename: "{app}\materials-workbench.exe"; AppUserModelID: "io.github.ember-moth.MaterialsWorkbench"; Tasks: desktopicon

[Registry]
; Among the applications in "Open with", not the default: Windows leaves
; that to the user.
Root: HKA; Subkey: "Software\Classes\.cif\OpenWithProgids"; ValueType: string; ValueName: "MaterialsWorkbench.cif"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\MaterialsWorkbench.cif"; ValueType: string; ValueName: ""; ValueData: "{cm:CifFile}"; Flags: uninsdeletekey; Tasks: associate
Root: HKA; Subkey: "Software\Classes\MaterialsWorkbench.cif\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\materials-workbench.exe,0"; Tasks: associate
Root: HKA; Subkey: "Software\Classes\MaterialsWorkbench.cif\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\materials-workbench.exe"" ""%1"""; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.vasp\OpenWithProgids"; ValueType: string; ValueName: "MaterialsWorkbench.vasp"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\MaterialsWorkbench.vasp"; ValueType: string; ValueName: ""; ValueData: "{cm:PoscarFile}"; Flags: uninsdeletekey; Tasks: associate
Root: HKA; Subkey: "Software\Classes\MaterialsWorkbench.vasp\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\materials-workbench.exe,0"; Tasks: associate
Root: HKA; Subkey: "Software\Classes\MaterialsWorkbench.vasp\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\materials-workbench.exe"" ""%1"""; Tasks: associate

[Run]
Filename: "{app}\materials-workbench.exe"; Description: "{cm:LaunchProgram,Materials Workbench}"; Flags: nowait postinstall skipifsilent

; Uninstalling removes the installed files only: settings, the copies of the
; sample projects and the user's projects stay.

[Code]
// The current user's PATH, for the "addtopath" task.
const
  EnvironmentKey = 'Environment';

function WrappedPath(): string;
var
  Paths: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', Paths) then
    Paths := '';
  Result := ';' + Paths + ';';
end;

procedure AddToPath(Dir: string);
var
  Wrapped: string;
begin
  Wrapped := WrappedPath();
  if Pos(';' + Uppercase(Dir) + ';', Uppercase(Wrapped)) > 0 then
    exit;
  StringChangeEx(Wrapped, ';;', ';', True);
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path',
    Copy(Wrapped, 2, Length(Wrapped) - 1) + Dir);
end;

procedure RemoveFromPath(Dir: string);
var
  Wrapped: string;
  At: Integer;
begin
  Wrapped := WrappedPath();
  At := Pos(';' + Uppercase(Dir) + ';', Uppercase(Wrapped));
  if At = 0 then
    exit;
  Delete(Wrapped, At, Length(Dir) + 1);
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path',
    Copy(Wrapped, 2, Length(Wrapped) - 2));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
