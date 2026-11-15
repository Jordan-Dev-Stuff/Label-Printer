; Label Printer installer (Inno Setup 6).
;
; Build (from the repository root, after fetching pdfium once):
;   powershell -ExecutionPolicy Bypass -File scripts\fetch-pdfium.ps1
;   cargo build --release --locked
;   iscc /DAppVersion=0.3.1 installer\label-printer.iss
; The installer lands in target\installer\. The release workflow
; (.github/workflows/release.yml) runs exactly these steps.
;
; Everything the app needs is inside label-printer.exe: the PDF renderer
; (pdfium) is embedded at build time, and printing uses only inbox Windows
; components (usbprint.sys, Bluetooth LE). No driver, no print queue, no
; download at install or run time. Per-user install, so no admin prompt.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define AppName "Label Printer"
#define AppExe "label-printer.exe"
#define Bin "..\target\release"

[Setup]
; Never change AppId: Windows uses it to find the existing install on upgrade.
; (Versions up to 0.2.0 were called thermal-rs; same AppId, so this upgrades them.)
AppId={{3B8E2F61-9C47-4D2A-A1E5-7F04C6B9D832}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Label Printer
DefaultDirName={autopf}\{#AppName}
; Ignore the old thermal-rs folder an earlier version was installed into.
UsePreviousAppDir=no
DefaultGroupName={#AppName}
UsePreviousGroup=no
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\installer
OutputBaseFilename=Label-Printer-{#AppVersion}-setup
SetupIconFile=..\assets\label-printer.ico
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
Name: "pdfopen"; Description: "Add ""Print with Label Printer"" to the right-click menu of PDF files"; GroupDescription: "Options:"; Flags: unchecked

[InstallDelete]
; Leftovers from versions named thermal-rs (0.2.0 and earlier).
Type: filesandordirs; Name: "{autopf}\thermal-rs"
Type: filesandordirs; Name: "{autoprograms}\thermal-rs"
Type: files; Name: "{autodesktop}\thermal-rs.lnk"

[Files]
Source: "{#Bin}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Bin}\tspl-rs.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\vendor\pdfium\windows-x86_64\LICENSE"; DestDir: "{app}"; DestName: "LICENSE-pdfium.txt"; Flags: ignoreversion

[Icons]
; The icon is embedded in the exe (crates/label-printer/build.rs).
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppExe}"; Comment: "Print labels without a driver"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Remove the right-click entry versions named thermal-rs added.
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.pdf\shell\thermal-rs"; Flags: deletekey
; Per-user "Print with Label Printer" verb for PDFs (HKCU, no admin).
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.pdf\shell\label-printer"; ValueType: string; ValueName: ""; ValueData: "Print with Label Printer"; Tasks: pdfopen; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.pdf\shell\label-printer"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"""; Tasks: pdfopen
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.pdf\shell\label-printer\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: pdfopen

[UninstallDelete]
; The unpacked copy of the embedded PDF renderer.
Type: filesandordirs; Name: "{localappdata}\label-printer\pdfium-*"

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start Label Printer"; Flags: nowait postinstall skipifsilent
