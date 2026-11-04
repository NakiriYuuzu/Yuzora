; yuzora(L7): Yuzora's Windows installer (docs/beta2/release.md), built by
; scripts/yuzora/package:
;
;   makensis -DVERSION=0.1.0-beta.2 -DVIVERSION=0.1.0.2 -DSRC=<yuzora folder>
;            -DICON=<yuzora.ico> -DPATHPS1=<path.ps1> -DOUT=<setup.exe> yuzora.nsi
;
; For one person, no admin: %LOCALAPPDATA%\Programs\Yuzora, the folder the
; app updates in place (src/update.rs); Yuzora in the Start menu; `yuzora` on
; the user PATH; an entry in Settings > Apps. The uninstaller sits beside it
; in Yuzora-uninstall, because an update replaces the whole Yuzora folder.
; Running it again replaces the app; an open Yuzora keeps running from the
; old folder until it quits. Your data (%USERPROFILE%\.config\yuzora) stays.
;
; Command line: /S silent, /D=<folder> elsewhere. The Tauri updater of Yuzora
; 0.1.0-beta.1 and older runs it as `/P /UPDATE /R /ARGS ...` to move people
; to this app: /P only shows progress, /R opens Yuzora afterwards.

Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define APP "Yuzora"
!define ID "dev.yuuzu.yuzora"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${ID}"
!define POWERSHELL "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"
; The Tauri app's MSI (0.0.18 to 0.1.0-beta.1), src-tauri/src/nsis_migration.rs.
!define TAURI_UPGRADE_CODE "{A5829978-F7B9-5882-B5BB-B8521FCEBF5F}"

Name "${APP}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
; Running it again replaces the install where it is, wherever /D= put it.
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
BrandingText "${APP} ${VERSION}"
VIProductVersion "${VIVERSION}"

Var Passive
Var Relaunch
; 1 when the folder is the one Yuzora's uninstall entry names (InstallLocation).
Var Ours

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_INSTFILES
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipWhenPassive
!define MUI_FINISHPAGE_RUN "$INSTDIR\yuzora.exe"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "TradChinese"

VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductName" "${APP}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileDescription" "${APP} installer"
VIAddVersionKey /LANG=${LANG_ENGLISH} "LegalCopyright" "Apache-2.0"

LangString OldApp ${LANG_ENGLISH} "An older Yuzora (0.1.0-beta.1 or before, installed for all users) is still on this PC. Remove it now? Removing it does not delete your old data, but the new Yuzora does not bring connections and settings over automatically yet, so you will need to set them up again."
LangString OldApp ${LANG_TRADCHINESE} "這台電腦還裝著舊版 Yuzora（0.1.0-beta.1 以前，為所有使用者安裝）。要現在移除嗎？移除不會刪掉舊版資料，但新版 Yuzora 目前還不會自動帶入連線與設定，需要重新設定。"
LangString NotOurs ${LANG_ENGLISH} "This doesn't look like a Yuzora folder, so nothing in it was removed:"
LangString NotOurs ${LANG_TRADCHINESE} "這看起來不是 Yuzora 的資料夾，所以裡面的檔案都沒有移除："
LangString NotEmpty ${LANG_ENGLISH} "Yuzora needs a folder of its own: updates replace the whole folder, and uninstalling removes it. This folder already has other files in it:"
LangString NotEmpty ${LANG_TRADCHINESE} "Yuzora 需要自己專用的資料夾：更新會替換整個資料夾，解除安裝也會把它刪掉。這個資料夾裡已經有其他檔案："

; Removes what the app made beside its folder (src/update.rs): only beside
; the folder its uninstall entry names (nothing of Yuzora's ran beside any
; other), and only with MARK in it.
!macro RemoveIfOurs DIR MARK
  ${If} $Ours == 1
  ${AndIf} ${FileExists} "${DIR}\${MARK}"
    RMDir /r "${DIR}"
  ${EndIf}
!macroend

Function .onInit
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/P" $R1
  ${IfNot} ${Errors}
    StrCpy $Passive 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/R" $R1
  ${IfNot} ${Errors}
    StrCpy $Relaunch 1
  ${EndIf}
  ${If} $Passive == 1
    SetAutoClose true
  ${EndIf}
FunctionEnd

Function SkipWhenPassive
  ${If} $Passive == 1
    Abort
  ${EndIf}
FunctionEnd

Function .onInstSuccess
  ${If} $Relaunch == 1
    Exec '"$INSTDIR\yuzora.exe"'
  ${EndIf}
FunctionEnd

; The Tauri app installed with its MSI (per machine): offer to remove it, so
; the Start menu has one Yuzora. msiexec asks for admin rights itself.
Function RemoveTauriApp
  System::Call 'msi::MsiEnumRelatedProductsW(w "${TAURI_UPGRADE_CODE}", i 0, i 0, w .R8) i .R7'
  ${If} $R7 != 0
    Return
  ${EndIf}
  MessageBox MB_YESNO|MB_ICONQUESTION "$(OldApp)" /SD IDNO IDNO done
  ExecWait '"$SYSDIR\msiexec.exe" /x $R8 /passive /norestart' $R6
  DetailPrint "msiexec /x $R8: $R6"
  done:
FunctionEnd

Section "Yuzora"
  ; Only into an empty folder or the one Yuzora is installed in, as its
  ; uninstall entry says: /D= can point anywhere, and a file that happens to
  ; be named yuzora.png proves nothing.
  ${DirState} "$INSTDIR" $R0
  ReadRegStr $R1 HKCU "${UNINST_KEY}" "InstallLocation"
  StrCpy $Ours 0
  ${If} $R1 == $INSTDIR
    StrCpy $Ours 1
  ${EndIf}
  ${If} $R0 == 1
  ${AndIf} $Ours != 1
    MessageBox MB_ICONSTOP "$(NotEmpty) $INSTDIR" /SD IDOK
    SetErrorLevel 2
    Quit
  ${EndIf}
  ; A running Yuzora keeps its program busy: move the old folder aside and
  ; install fresh. The old one is removed below; the files the running app
  ; still holds stay until the next install or update.
  ${If} ${FileExists} "$INSTDIR\yuzora.exe"
    !insertmacro RemoveIfOurs "$INSTDIR.old" "yuzora.exe"
    ClearErrors
    Delete "$INSTDIR\yuzora.exe"
    ${If} ${Errors}
      Rename "$INSTDIR" "$INSTDIR.old"
    ${EndIf}
  ${EndIf}
  SetOutPath "$INSTDIR"
  File /r "${SRC}\*.*"
  ; What the app staged for the version this replaces (src/update.rs).
  !insertmacro RemoveIfOurs "$INSTDIR.new" "yuzora.png"
  ${If} $Ours == 1
    Delete "$INSTDIR.new.json"
  ${EndIf}
  !insertmacro RemoveIfOurs "$INSTDIR.download" "app.tar.gz"
  !insertmacro RemoveIfOurs "$INSTDIR.old" "yuzora.exe"

  SetOutPath "$INSTDIR-uninstall"
  File "/oname=path.ps1" "${PATHPS1}"
  WriteUninstaller "$INSTDIR-uninstall\uninstall.exe"
  SetOutPath "$INSTDIR"
  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\yuzora.exe"
  nsExec::Exec '"${POWERSHELL}" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR-uninstall\path.ps1" -Add "$INSTDIR"'
  Pop $0
  DetailPrint "PATH: $0"

  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Yuuzu"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\yuzora.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://github.com/NakiriYuuzu/Yuzora"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR-uninstall\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR-uninstall\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $0

  Call RemoveTauriApp
SectionEnd

Section "Uninstall"
  ; $INSTDIR is the uninstaller's own folder, <app folder>-uninstall, and
  ; the app folder must be the one Yuzora's uninstall entry names.
  StrCpy $0 $INSTDIR "" -10
  StrCpy $1 $INSTDIR -10
  ReadRegStr $2 HKCU "${UNINST_KEY}" "InstallLocation"
  ${If} $0 != "-uninstall"
  ${OrIf} $2 != $1
  ${OrIfNot} ${FileExists} "$1\yuzora.png"
    MessageBox MB_ICONEXCLAMATION "$(NotOurs) $1" /SD IDOK
    Abort
  ${EndIf}
  StrCpy $Ours 1
  nsExec::Exec '"${POWERSHELL}" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\path.ps1" -Remove "$1"'
  Pop $2
  Delete "$SMPROGRAMS\${APP}.lnk"
  RMDir /r "$1"
  !insertmacro RemoveIfOurs "$1.old" "yuzora.exe"
  !insertmacro RemoveIfOurs "$1.new" "yuzora.png"
  !insertmacro RemoveIfOurs "$1.download" "app.tar.gz"
  Delete "$1.new.json"
  DeleteRegKey HKCU "${UNINST_KEY}"
  Delete "$INSTDIR\path.ps1"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd
