; thermal-rs installer (Inno Setup 6).
;
; Build (from the repository root, after fetching pdfium once):
;   powershell -ExecutionPolicy Bypass -File scripts\fetch-pdfium.ps1
;   cargo build --release --locked
;   iscc /DAppVersion=0.2.0 installer\thermal-rs.iss
; The installer lands in target\installer\. The release workflow
; (.github/workflows/release.yml) runs exactly these steps.
;
; Everything the app needs is inside thermal-ui.exe: the PDF renderer
; (pdfium) is embedded at build time, and printing uses only inbox Windows
; components (usbprint.sys, Bluetooth LE). No driver, no print queue, no
; download at install or run time. Per-user install, so no admin prompt.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppName "thermal-rs"
#define AppExe "thermal-ui.exe"
#define Bin "..\target\release"

[Setup]
; Never change AppId: Windows uses it to find the existing install on upgrade.
AppId={{3B8E2F61-9C47-4D2A-A1E5-7F04C6B9D832}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=thermal-rs
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\installer
OutputBaseFilename=thermal-rs-{#AppVersion}-setup
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Options:"; Flags: unchecked
Name: "pdfopen"; Description: "Add ""Print label with thermal-rs"" to the right-click menu of PDF files"; GroupDescription: "Options:"; Flags: unchecked

[Files]
Source: "{#Bin}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Bin}\tspl-rs.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\vendor\pdfium\windows-x86_64\LICENSE"; DestDir: "{app}"; DestName: "LICENSE-pdfium.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\thermal-rs"; Filename: "{app}\{#AppExe}"; Comment: "Print labels without a driver"
Name: "{autodesktop}\thermal-rs"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Per-user "Print label with thermal-rs" verb for PDFs (HKCU, no admin).
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.pdf\shell\thermal-rs"; ValueType: string; ValueName: ""; ValueData: "Print label with thermal-rs"; Tasks: pdfopen; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.pdf\shell\thermal-rs"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"""; Tasks: pdfopen
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.pdf\shell\thermal-rs\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: pdfopen

[UninstallDelete]
; The unpacked copy of the embedded PDF renderer.
Type: filesandordirs; Name: "{localappdata}\thermal-rs\pdfium-*"

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start thermal-rs"; Flags: nowait postinstall skipifsilent
