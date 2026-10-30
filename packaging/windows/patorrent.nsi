; PaTorrent's Windows installer: NSIS 3, Unicode, Modern UI 2, x64 only, Windows 10 or later.
; How to build it and what it does: packaging/windows/README.md. The release workflow builds it
; from the release executables beside the portable zip (docs/RELEASING.md):
;
;   makensis /DVERSION=1.0.0-rc.1 /DBINDIR=target\release
;     /DICON=assets\patina-icons\exports\app\01-verdigris.ico
;     /DOUTFILE=patorrent-1.0.0-rc.1-windows-x64-setup.exe packaging\windows\patorrent.nsi
;
; - For one user (the default) it installs into %LOCALAPPDATA%\Programs\PaTorrent without
;   elevation, as uTorrent did; for all users into Program Files. MultiUser.nsh cannot raise its
;   rights halfway (with RequestExecutionLevel highest every administrator gets a UAC prompt
;   even for a per-user install), so this runs as the user and, when all users is chosen,
;   starts itself again with ShellExecuteEx "runas" and waits for that copy (RunElevated).
; - The mode and folder of an installation are its Uninstall key: HKCU for one user, HKLM for
;   all. An upgrade finds that key and installs over it, in the same mode and folder.
; - The associations are patorrent.exe's own (--associate, --unregister in
;   crates/win32/src/main.rs, written by integration.rs under HKCU), never written here.
; - Nothing running is killed: PaTorrent's window or a busy patorrent-nox.exe (the PaTorrent
;   nox service, or a console) gets Retry / Cancel.
;
; Switches besides NSIS's own /S and /D=<folder> (which must come last):
;   /CURRENTUSER, /ALLUSERS     the mode, without asking (default: the installation found, else
;                               one user)
;   /DESKTOPSHORTCUT            a desktop shortcut (silent installs make none otherwise)
;   /ASSOCIATE=torrent,magnet   associate those (silent installs and upgrades leave them)
;   /UPDATE=<pid>               PaTorrent's own update (D45): wait up to a minute for that
;                               process (the PaTorrent that downloaded this setup and is
;                               exiting) before checking the files, and start patorrent.exe
;                               again at the end, as the user, whether the install succeeded or
;                               not. Meant with /S.
; and internal ones, for the elevated copy: /ELEVATED, /CONTINUE, /LANG=<language id>.

Unicode true

!macro REQUIRE NAME
  !ifndef ${NAME}
    !error "${NAME} is not set: makensis /D${NAME}=... (packaging/windows/README.md)"
  !endif
!macroend
!insertmacro REQUIRE VERSION
!insertmacro REQUIRE BINDIR
!insertmacro REQUIRE ICON
!insertmacro REQUIRE OUTFILE

; makensis moves into the script's folder unless /NOCD, while __FILEDIR__ is the script's folder
; as the command line named it: with /NOCD (or an absolute script path) ROOT is the repository
!define ROOT "${__FILEDIR__}\..\.."
!if ! /FileExists "${ROOT}\LICENSE.md"
  !error "${ROOT}\LICENSE.md is missing: run makensis /NOCD from the repository (packaging/windows/README.md)"
!endif
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\PaTorrent"
; Windows wants four numbers: 1.0.0-rc.1 is 1.0.0.0, as in the Windows manifest
!searchparse "${VERSION}-" "" VERSION_MAJOR "." VERSION_MINOR "." VERSION_PATCH "-"
!define VERSION_QUAD "${VERSION_MAJOR}.${VERSION_MINOR}.${VERSION_PATCH}.0"

Name "PaTorrent"
OutFile "${OUTFILE}"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ManifestDPIAware true
ManifestSupportedOS Win10
BrandingText "PaTorrent ${VERSION}"

!include MUI2.nsh
!include LogicLib.nsh
!include FileFunc.nsh
!include Sections.nsh
!include WinVer.nsh
!include x64.nsh
!include nsDialogs.nsh
!include Integration.nsh
!include StrFunc.nsh
${Using:StrFunc} StrStr

Var Mode        ; "user" or "all"
Var ModeFixed   ; 1: a switch or the installation found decided the mode (no Users page)
Var IsAdmin     ; 1: this process is elevated
Var Elevated    ; 1: started elevated by another copy of itself (/ELEVATED)
Var Continue    ; 1: that copy showed Welcome, License and Users already (/CONTINUE)
Var LangGiven   ; 1: /LANG= chose the language
Var DirGiven    ; 1: /D= chose the folder
Var GivenDir
Var PrevDir     ; the installation found for the mode, if any
Var Upgrade     ; 1: installing over it (its folder, no Directory page)
Var OptDesktop  ; 1: /DESKTOPSHORTCUT
Var AssocGiven  ; 1: /ASSOCIATE= was given
Var OptAssoc
Var AssocList   ; what the association sections ask patorrent.exe for
Var UpdatePid   ; /UPDATE=: the exiting PaTorrent to wait for; "" otherwise
Var UsersMe
Var UsersAll

;--------------------------------
; Pages

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_UNABORTWARNING
!define MUI_COMPONENTSPAGE_NODESC
!define MUI_LANGDLL_ALLLANGUAGES

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfContinue
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfContinue
!define MUI_PAGE_CUSTOMFUNCTION_SHOW ShieldOff
!insertmacro MUI_PAGE_LICENSE "${ROOT}\LICENSE.md"
Page custom UsersPage UsersPageLeave
!insertmacro MUI_PAGE_COMPONENTS
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfUpgrade
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchApp
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_COMPONENTS
!insertmacro MUI_UNPAGE_INSTFILES

;--------------------------------
; Languages: English first (the fallback), then the app's other 33 (crates/frontend/lang)

!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Arabic"
!insertmacro MUI_LANGUAGE "Bulgarian"
!insertmacro MUI_LANGUAGE "Catalan"
!insertmacro MUI_LANGUAGE "Czech"
!insertmacro MUI_LANGUAGE "Danish"
!insertmacro MUI_LANGUAGE "German"
!insertmacro MUI_LANGUAGE "Greek"
!insertmacro MUI_LANGUAGE "Spanish"
!insertmacro MUI_LANGUAGE "Farsi"
!insertmacro MUI_LANGUAGE "Finnish"
!insertmacro MUI_LANGUAGE "French"
!insertmacro MUI_LANGUAGE "Hebrew"
!insertmacro MUI_LANGUAGE "Croatian"
!insertmacro MUI_LANGUAGE "Hungarian"
!insertmacro MUI_LANGUAGE "Indonesian"
!insertmacro MUI_LANGUAGE "Italian"
!insertmacro MUI_LANGUAGE "Japanese"
!insertmacro MUI_LANGUAGE "Korean"
!insertmacro MUI_LANGUAGE "Norwegian"
!insertmacro MUI_LANGUAGE "Dutch"
!insertmacro MUI_LANGUAGE "Polish"
!insertmacro MUI_LANGUAGE "PortugueseBR"
!insertmacro MUI_LANGUAGE "Portuguese"
!insertmacro MUI_LANGUAGE "Romanian"
!insertmacro MUI_LANGUAGE "Russian"
!insertmacro MUI_LANGUAGE "Slovak"
!insertmacro MUI_LANGUAGE "Serbian"
!insertmacro MUI_LANGUAGE "Swedish"
!insertmacro MUI_LANGUAGE "Thai"
!insertmacro MUI_LANGUAGE "Turkish"
!insertmacro MUI_LANGUAGE "Ukrainian"
!insertmacro MUI_LANGUAGE "Vietnamese"
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "TradChinese"
!insertmacro MUI_RESERVEFILE_LANGDLL

!include /CHARSET=UTF8 "${__FILEDIR__}\strings.nsh"

VIProductVersion "${VERSION_QUAD}"
VIFileVersion "${VERSION_QUAD}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductName" "PaTorrent"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileDescription" "PaTorrent Setup"
VIAddVersionKey /LANG=${LANG_ENGLISH} "CompanyName" "DukeCorp1996"
VIAddVersionKey /LANG=${LANG_ENGLISH} "LegalCopyright" "Copyright 2026 DukeCorp1996"

;--------------------------------
; Shared by the installer and the uninstaller

; Starts this program again as administrator with $0 as its arguments and waits for it: $1 is
; its exit code, or "" when it did not start (the UAC prompt was declined).
!macro RUN_ELEVATED UN
Function ${UN}RunElevated
  StrCpy $1 ""
  ; SHELLEXECUTEINFO with SEE_MASK_NOCLOSEPROCESS, so its hProcess can be waited for
  System::Call '*(&l4, i 0x40, p $HWNDPARENT, t "runas", t "$EXEPATH", t "$0", p 0, i 1, p, p, p, p, i, p, p) p .r2'
  System::Call 'shell32::ShellExecuteEx(p r2) i .r3'
  ${If} $3 <> 0
    System::Call '*$2(i, i, p, p, p, p, p, i, p, p, p, p, i, p, p .r3)'
    System::Call 'kernel32::WaitForSingleObject(p r3, i -1)'
    System::Call 'kernel32::GetExitCodeProcess(p r3, *i .r1)'
    System::Call 'kernel32::CloseHandle(p r3)'
  ${EndIf}
  System::Free $2
FunctionEnd
!macroend
!insertmacro RUN_ELEVATED ""
!insertmacro RUN_ELEVATED "un."

; Whether $0 exists and cannot be opened for writing, as a running program cannot: $1 = 1.
!macro IN_USE UN
Function ${UN}InUse
  StrCpy $1 0
  ${If} ${FileExists} $0
    ClearErrors
    FileOpen $2 $0 a
    ${If} ${Errors}
      StrCpy $1 1
    ${Else}
      FileClose $2
    ${EndIf}
  ${EndIf}
FunctionEnd
!macroend
!insertmacro IN_USE ""
!insertmacro IN_USE "un."

; Before the files are replaced or removed: PaTorrent must not be running (its window, or its
; executable busy in another session), nor patorrent-nox.exe from this folder (the service, or a
; console); the uninstaller also wants the service removed first, as it would be left starting
; a deleted executable. Retry or Cancel; nothing is killed.
!macro CHECK_FILES UN
Function ${UN}CheckFiles
  app:
    FindWindow $1 "PaTorrentMainWnd"
    ${If} $1 = 0
      FindWindow $1 "PaTorrentRemoteWnd"
    ${EndIf}
    ${If} $1 = 0
      StrCpy $0 "$INSTDIR\patorrent.exe"
      Call ${UN}InUse
    ${EndIf}
    ${If} $1 <> 0
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(PT_RUNNING)" /SD IDCANCEL IDRETRY app
      Abort
    ${EndIf}
  !if "${UN}" == "un."
  service:
    ; `service install` writes the image path as "<exe>" service run ...
    ReadRegStr $1 HKLM "SYSTEM\CurrentControlSet\Services\PaTorrentNox" "ImagePath"
    StrCpy $2 '"$INSTDIR\patorrent-nox.exe"'
    StrLen $3 $2
    StrCpy $1 $1 $3
    ${If} $1 == $2
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(PT_NOX_SERVICE)" /SD IDCANCEL IDRETRY service
      Abort
    ${EndIf}
  !endif
  nox:
    StrCpy $0 "$INSTDIR\patorrent-nox.exe"
    Call ${UN}InUse
    ${If} $1 = 1
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(PT_NOX_RUNNING)" /SD IDCANCEL IDRETRY nox
      Abort
    ${EndIf}
FunctionEnd
!macroend
!insertmacro CHECK_FILES ""
!insertmacro CHECK_FILES "un."

!macro IS_ADMIN
  UserInfo::GetAccountType
  Pop $0
  ${If} $0 == "Admin"
    StrCpy $IsAdmin 1
  ${Else}
    StrCpy $IsAdmin 0
  ${EndIf}
!macroend

;--------------------------------
; Installer

Section "PaTorrent" SecMain
  SectionIn RO
  Call CheckFiles
  SetOutPath "$INSTDIR"
  File "${BINDIR}\patorrent.exe"
  File "${BINDIR}\patorrent-nox.exe"
  File "${ROOT}\LICENSE.md"
  File "${ROOT}\THIRD-PARTY-NOTICES.md"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\PaTorrent.lnk" "$INSTDIR\patorrent.exe"

  ; Settings > Apps (Add/Remove Programs); SHCTX is HKCU or HKLM, as SetShellVarContext
  WriteRegStr SHCTX "${UNINST_KEY}" "DisplayName" "PaTorrent"
  WriteRegStr SHCTX "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINST_KEY}" "Publisher" "DukeCorp1996"
  WriteRegStr SHCTX "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\patorrent.exe,0"
  WriteRegStr SHCTX "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr SHCTX "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr SHCTX "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr SHCTX "${UNINST_KEY}" "URLInfoAbout" "https://balls0fsteel.github.io/PaTorrent/"
  WriteRegStr SHCTX "${UNINST_KEY}" "HelpLink" "https://github.com/Balls0fSteel/PaTorrent/releases"
  WriteRegStr SHCTX "${UNINST_KEY}" "URLUpdateInfo" "https://github.com/Balls0fSteel/PaTorrent/releases"
  WriteRegDWORD SHCTX "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD SHCTX "${UNINST_KEY}" "NoRepair" 1
  ; the uninstaller speaks the language chosen here
  WriteRegStr SHCTX "${UNINST_KEY}" "InstallerLanguage" $LANGUAGE
SectionEnd

Section "$(PT_DESKTOP)" SecDesktop
  CreateShortcut "$DESKTOP\PaTorrent.lnk" "$INSTDIR\patorrent.exe"
SectionEnd

Section "$(PT_ASSOC_TORRENT)" SecTorrent
  StrCpy $AssocList "torrent"
SectionEnd

Section "$(PT_ASSOC_MAGNET)" SecMagnet
  ${If} $AssocList == ""
    StrCpy $AssocList "magnet"
  ${Else}
    StrCpy $AssocList "$AssocList,magnet"
  ${EndIf}
SectionEnd

Section "-Finish"
  ; unticked on an upgrade: the shortcut the last installation made goes
  ${If} $Upgrade == 1
  ${AndIfNot} ${SectionIsSelected} ${SecDesktop}
    Delete "$DESKTOP\PaTorrent.lnk"
  ${EndIf}
  ; for the user running Setup (HKCU): elevated with another account's password, that account
  ${If} $AssocList != ""
    ExecWait '"$INSTDIR\patorrent.exe" --associate $AssocList' $0
    DetailPrint "patorrent.exe --associate $AssocList: $0"
  ${EndIf}
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD SHCTX "${UNINST_KEY}" "EstimatedSize" $0
SectionEnd

Function .onInit
  SetRegView 64
  ; x64 programs run on Arm64 from Windows 11 on
  StrCpy $0 0
  ${If} ${AtLeastWin10}
    ${If} ${IsNativeAMD64}
      StrCpy $0 1
    ${ElseIf} ${IsNativeARM64}
    ${AndIf} ${AtLeastWin11}
      StrCpy $0 1
    ${EndIf}
  ${EndIf}
  ${If} $0 <> 1
    MessageBox MB_ICONSTOP "$(PT_NEEDS)" /SD IDOK
    SetErrorLevel 1
    Quit
  ${EndIf}

  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/LANG=" $R1
  ${IfNot} ${Errors}
    StrCpy $LANGUAGE $R1
    StrCpy $LangGiven 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/ELEVATED" $R1
  ${IfNot} ${Errors}
    StrCpy $Elevated 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/CONTINUE" $R1
  ${IfNot} ${Errors}
    StrCpy $Continue 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/CURRENTUSER" $R1
  ${IfNot} ${Errors}
    StrCpy $Mode user
    StrCpy $ModeFixed 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/ALLUSERS" $R1
  ${IfNot} ${Errors}
    StrCpy $Mode all
    StrCpy $ModeFixed 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/DESKTOPSHORTCUT" $R1
  ${IfNot} ${Errors}
    StrCpy $OptDesktop 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/ASSOCIATE=" $R1
  ${IfNot} ${Errors}
    StrCpy $AssocGiven 1
    StrCpy $OptAssoc $R1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/UPDATE=" $R1
  ${IfNot} ${Errors}
    StrCpy $UpdatePid $R1
    Call WaitForUpdatePid
  ${EndIf}
  ${If} $INSTDIR != ""
    StrCpy $DirGiven 1
    StrCpy $GivenDir $INSTDIR
  ${EndIf}
  !insertmacro IS_ADMIN

  ; without a switch, the installation found decides: one user's first, then all users'
  ${If} $ModeFixed != 1
    ReadRegStr $0 HKCU "${UNINST_KEY}" "InstallLocation"
    ReadRegStr $1 HKLM "${UNINST_KEY}" "InstallLocation"
    ${If} $0 != ""
      StrCpy $Mode user
      StrCpy $ModeFixed 1
    ${ElseIf} $1 != ""
      StrCpy $Mode all
      StrCpy $ModeFixed 1
    ${Else}
      StrCpy $Mode user
    ${EndIf}
  ${EndIf}
  Call ApplyMode

  ${If} $LangGiven != 1
    !insertmacro MUI_LANGDLL_DISPLAY
  ${EndIf}

  ; all users decided already: elevate now (the elevated copy shows every page)
  ${If} $Mode == all
  ${AndIf} $IsAdmin != 1
    StrCpy $R1 ""
    Call Elevate
    SetErrorLevel 740 ; ERROR_ELEVATION_REQUIRED
    Quit
  ${EndIf}
  Call PresetSections
FunctionEnd

; The shell folders, the default folder and whether this is an upgrade, for $Mode.
Function ApplyMode
  ${If} $Mode == all
    SetShellVarContext all
    ReadRegStr $PrevDir HKLM "${UNINST_KEY}" "InstallLocation"
    StrCpy $0 "$PROGRAMFILES64\PaTorrent"
  ${Else}
    SetShellVarContext current
    ReadRegStr $PrevDir HKCU "${UNINST_KEY}" "InstallLocation"
    StrCpy $0 "$LOCALAPPDATA\Programs\PaTorrent"
  ${EndIf}
  ${If} $DirGiven == 1
    StrCpy $INSTDIR $GivenDir
  ${ElseIf} $PrevDir != ""
    StrCpy $INSTDIR $PrevDir
  ${Else}
    StrCpy $INSTDIR $0
  ${EndIf}
  StrCpy $Upgrade 0
  ${If} $PrevDir != ""
  ${AndIf} $PrevDir == $INSTDIR
    StrCpy $Upgrade 1
  ${EndIf}
FunctionEnd

; The optional sections' first state. Desktop shortcut: as it is on an upgrade, ticked on a new
; installation, none when silent. Associations: ticked on a new installation; an upgrade or a
; silent install leaves them as they are. /DESKTOPSHORTCUT and /ASSOCIATE= override.
Function PresetSections
  ${If} $OptDesktop == 1
    !insertmacro SelectSection ${SecDesktop}
  ${ElseIf} $Upgrade == 1
    ${If} ${FileExists} "$DESKTOP\PaTorrent.lnk"
      !insertmacro SelectSection ${SecDesktop}
    ${Else}
      !insertmacro UnselectSection ${SecDesktop}
    ${EndIf}
  ${ElseIf} ${Silent}
    !insertmacro UnselectSection ${SecDesktop}
  ${EndIf}
  ${If} $AssocGiven == 1
    !insertmacro UnselectSection ${SecTorrent}
    !insertmacro UnselectSection ${SecMagnet}
    ${StrStr} $0 $OptAssoc "torrent"
    ${If} $0 != ""
      !insertmacro SelectSection ${SecTorrent}
    ${EndIf}
    ${StrStr} $0 $OptAssoc "magnet"
    ${If} $0 != ""
      !insertmacro SelectSection ${SecMagnet}
    ${EndIf}
  ${ElseIf} $Upgrade == 1
  ${OrIf} ${Silent}
    !insertmacro UnselectSection ${SecTorrent}
    !insertmacro UnselectSection ${SecMagnet}
  ${EndIf}
FunctionEnd

; All users needs administrator rights: runs the elevated copy with what this one knows ($R1:
; flags of the caller's), waits for it and quits with its exit code. Returns only when it did
; not start, after saying so.
Function Elevate
  ${If} $Elevated != 1 ; elevated and still not an administrator: no second try
    StrCpy $0 "/ELEVATED /ALLUSERS /LANG=$LANGUAGE $R1"
    ${If} ${Silent}
      StrCpy $0 "/S $0"
    ${EndIf}
    ${If} $OptDesktop == 1
      StrCpy $0 "$0 /DESKTOPSHORTCUT"
    ${EndIf}
    ${If} $AssocGiven == 1
      StrCpy $0 "$0 /ASSOCIATE=$OptAssoc"
    ${EndIf}
    ${If} $DirGiven == 1
      StrCpy $0 "$0 /D=$INSTDIR"
    ${EndIf}
    HideWindow
    Call RunElevated
    ; an update starts PaTorrent again from this copy, which runs as the user: the new version
    ; after a successful install, else the one still there
    ${If} $UpdatePid != ""
      Call LaunchApp
    ${EndIf}
    ${If} $1 != ""
      SetErrorLevel $1
      Quit
    ${EndIf}
    BringToFront
  ${EndIf}
  MessageBox MB_ICONEXCLAMATION "$(PT_ADMIN)" /SD IDOK
FunctionEnd

; /UPDATE=<pid>: waits up to a minute for that process to exit, so the files are free. A
; process that is gone already, or cannot be opened, is not waited for; one still running after
; the minute is CheckFiles' business (Retry / Cancel; silent: Cancel).
Function WaitForUpdatePid
  ; SYNCHRONIZE
  System::Call 'kernel32::OpenProcess(i 0x00100000, i 0, i $UpdatePid) p .r0'
  ${If} $0 P<> 0
    System::Call 'kernel32::WaitForSingleObject(p r0, i 60000)'
    System::Call 'kernel32::CloseHandle(p r0)'
  ${EndIf}
FunctionEnd

; After an update's install (this copy's own, as one user): PaTorrent again. A failed one (the
; files still busy, say) leaves the old version in place, so that is what starts.
Function .onInstSuccess
  ${If} $UpdatePid != ""
    Call LaunchApp
  ${EndIf}
FunctionEnd

Function .onInstFailed
  ${If} $UpdatePid != ""
    Call LaunchApp
  ${EndIf}
FunctionEnd

Function SkipIfContinue
  ${If} $Continue == 1
    Abort
  ${EndIf}
FunctionEnd

Function SkipIfUpgrade
  ${If} $Upgrade == 1
    Abort
  ${EndIf}
FunctionEnd

; The UAC shield on Next while it would elevate, as Windows' own dialogs show it.
Function Shield
  GetDlgItem $1 $HWNDPARENT 1
  StrCpy $2 0
  ${If} $IsAdmin != 1
    ${NSD_GetState} $UsersAll $0
    ${If} $0 == ${BST_CHECKED}
      StrCpy $2 1
    ${EndIf}
  ${EndIf}
  SendMessage $1 ${BCM_SETSHIELD} 0 $2
FunctionEnd

Function ShieldOff
  GetDlgItem $1 $HWNDPARENT 1
  SendMessage $1 ${BCM_SETSHIELD} 0 0
FunctionEnd

Function UsersPage
  ${If} $ModeFixed == 1
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "$(PT_USERS_TITLE)" "$(PT_USERS_SUBTITLE)"
  nsDialogs::Create 1018
  Pop $0
  ${NSD_CreateRadioButton} 0 0 100% 12u "$(PT_USERS_ME)"
  Pop $UsersMe
  ${NSD_AddStyle} $UsersMe ${WS_GROUP}
  ${NSD_CreateRadioButton} 0 18u 100% 12u "$(PT_USERS_ALL)"
  Pop $UsersAll
  ${NSD_OnClick} $UsersMe UsersClick
  ${NSD_OnClick} $UsersAll UsersClick
  ${If} $Mode == all
    ${NSD_Check} $UsersAll
  ${Else}
    ${NSD_Check} $UsersMe
  ${EndIf}
  Call Shield
  nsDialogs::Show
FunctionEnd

Function UsersClick
  Pop $0
  Call Shield
FunctionEnd

Function UsersPageLeave
  Call ShieldOff
  ${NSD_GetState} $UsersAll $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $Mode all
  ${Else}
    StrCpy $Mode user
  ${EndIf}
  Call ApplyMode
  Call PresetSections
  ${If} $Mode == all
  ${AndIf} $IsAdmin != 1
    StrCpy $R1 "/CONTINUE"
    Call Elevate
    Call Shield
    Abort ; stay here: "just me" still works
  ${EndIf}
FunctionEnd

; Run PaTorrent from the Finish page (never in a silent install, which has no pages). An
; elevated installer would start it elevated too; Explorer starts it as the signed-in user.
Function LaunchApp
  ${If} $IsAdmin == 1
    Exec '"$WINDIR\explorer.exe" "$INSTDIR\patorrent.exe"'
  ${Else}
    Exec '"$INSTDIR\patorrent.exe"'
  ${EndIf}
FunctionEnd

;--------------------------------
; Uninstaller

Function un.onInit
  SetRegView 64
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/LANG=" $R1
  ${IfNot} ${Errors}
    StrCpy $LANGUAGE $R1
    StrCpy $LangGiven 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/ELEVATED" $R1
  ${IfNot} ${Errors}
    StrCpy $Elevated 1
  ${EndIf}
  !insertmacro IS_ADMIN

  ; all users' when HKLM's key names this folder
  ReadRegStr $0 HKLM "${UNINST_KEY}" "InstallLocation"
  ${If} $0 != ""
  ${AndIf} $0 == $INSTDIR
    StrCpy $Mode all
    SetShellVarContext all
  ${Else}
    StrCpy $Mode user
    SetShellVarContext current
  ${EndIf}

  ${If} $LangGiven != 1
    ReadRegStr $0 SHCTX "${UNINST_KEY}" "InstallerLanguage"
    ${If} $0 != ""
      StrCpy $LANGUAGE $0
    ${Else}
      !insertmacro MUI_LANGDLL_DISPLAY
    ${EndIf}
  ${EndIf}

  ${If} $Mode == all
  ${AndIf} $IsAdmin != 1
    ${If} $Elevated != 1
      ; _?= keeps the copy in %TEMP% that is running now, so it can delete uninstall.exe
      StrCpy $0 "/ELEVATED /LANG=$LANGUAGE _?=$INSTDIR"
      ${If} ${Silent}
        StrCpy $0 "/S $0"
      ${EndIf}
      Call un.RunElevated
      ${If} $1 != ""
        SetErrorLevel $1
        Quit
      ${EndIf}
    ${EndIf}
    MessageBox MB_ICONSTOP "$(PT_ADMIN)" /SD IDOK
    SetErrorLevel 740 ; ERROR_ELEVATION_REQUIRED
    Quit
  ${EndIf}
FunctionEnd

Section "un.PaTorrent" SecUnMain
  SectionIn RO
  Call un.CheckFiles
  ; associations, Default apps and autostart that name this patorrent.exe (integration.rs)
  ExecWait '"$INSTDIR\patorrent.exe" --unregister' $0
  DetailPrint "patorrent.exe --unregister: $0"
  ${UnpinShortcut} "$SMPROGRAMS\PaTorrent.lnk"
  Delete "$SMPROGRAMS\PaTorrent.lnk"
  Delete "$DESKTOP\PaTorrent.lnk"
  Delete "$INSTDIR\patorrent.exe"
  Delete "$INSTDIR\patorrent-nox.exe"
  Delete "$INSTDIR\LICENSE.md"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.md"
  Delete "$INSTDIR\uninstall.exe"
  ; only when empty: a portable patorrent.ini or anything else put here stays
  RMDir "$INSTDIR"
  DeleteRegKey SHCTX "${UNINST_KEY}"
SectionEnd

; Unticked by default, as uTorrent's uninstaller had it. What PaTorrent keeps in
; %APPDATA%\PaTorrent (the user's who runs this), by name: the GUI's settings, its torrent list
; and resume data (state), RSS feeds, remote connections and their windows' folders, the default
; IP filter, and nox's state when it ran from a console. The folder itself goes only when that
; leaves it empty, so a download folder someone put inside it stays.
Section /o "un.$(PT_UN_DATA)" SecUnData
  SetShellVarContext current
  StrCpy $0 "$APPDATA\PaTorrent"
  Delete "$0\patorrent.ini"
  Delete "$0\patorrent.tmp"
  Delete "$0\remote.ini"
  Delete "$0\remote.tmp"
  Delete "$0\rss.dat"
  Delete "$0\rss.tmp"
  Delete "$0\ipfilter.dat"
  RMDir /r "$0\state"
  RMDir /r "$0\remote"
  RMDir /r "$0\nox"
  RMDir "$0"
  ${If} $Mode == all
    SetShellVarContext all
  ${EndIf}
SectionEnd
