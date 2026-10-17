; RadioMaster installer (Inno Setup 6)
; Build:  iscc /DAppVersion=0.2.0 installer\RadioMaster.iss   (after "cargo build --release")
; Output: dist\RadioMaster-Setup-<version>.exe

#ifndef AppVersion
  #define AppVersion "0.2.3"
#endif

#define AppName "RadioMaster"
#define AppExe "RadioMaster.exe"

[Setup]
AppId={{6D7DB8E2-88CE-4D8E-B734-1FBC3A2350FB}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=DovaFM Digital Entertainment Group
AppPublisherURL=https://deg.dovafm.com
AppSupportURL=https://github.com/epell-development/RadioMaster/issues
AppUpdatesURL=https://github.com/epell-development/RadioMaster/releases
VersionInfoCompany=DovaFM Digital Entertainment Group
AppComments=Radio automation for Icecast, Shoutcast and AzuraCast
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
OutputDir=..\dist
OutputBaseFilename=RadioMaster-Setup-{#AppVersion}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
WizardImageFile=wizard-large.bmp,wizard-large-2x.bmp
WizardSmallImageFile=wizard-small.bmp,wizard-small-2x.bmp
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; per-user or all-users install, chosen by the user
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\PLUGINS.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\lang\*.json"; DestDir: "{app}\lang"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autoprograms}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
; updates started from inside RadioMaster run silently: start it again when they finish
Filename: "{app}\{#AppExe}"; Flags: nowait runasoriginaluser; Check: ShouldRelaunch

; Settings, playlists and logs in %APPDATA%\RadioMaster are kept on uninstall so a reinstall picks them up.

[Code]
// RadioMaster's own updater passes /RELAUNCH=1 so it starts again after a silent update.
function ShouldRelaunch(): Boolean;
begin
  Result := WizardSilent() and (ExpandConstant('{param:RELAUNCH|0}') = '1');
end;

// Embedded web panels need the Edge WebView2 runtime (part of Windows 11 and up-to-date Windows 10).
function WebView2Installed(): Boolean;
var v: String;
begin
  Result :=
    RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', v) or
    RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', v) or
    RegQueryStringValue(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', v);
  if Result then Result := (v <> '') and (v <> '0.0.0.0');
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = wpFinished) and (not WizardSilent()) and (not WebView2Installed()) then
    MsgBox('RadioMaster is installed.' + #13#10#13#10 +
      'The Microsoft Edge WebView2 Runtime was not found. Everything works without it except the web panels (Web 1-4).' + #13#10 +
      'You can get it free from Microsoft: https://developer.microsoft.com/microsoft-edge/webview2/', mbInformation, MB_OK);
end;
