; syodep Windows installer.
;
; A thin wrapper over the portable tree that release.yml has already staged and
; smoke-tested -- this script installs files, it never builds them.
;
; Build (see .github/workflows/release.yml):
;   makensis -WX -DVERSION=0.4.0 -DVERSION_NUMERIC=0.4.0.0 \
;            -DSRCDIR=syodep-win64 -DLICENSE_FILE=license-crlf.txt syodep.nsi
;
; Deliberate choices, all of which have bitten someone before:
;   * Per-user install. No UAC, works on locked-down machines, and it is what
;     lets CI verify a real install/uninstall round trip without elevation.
;   * The uninstaller does NOT touch %APPDATA%\syodep. That directory is shared
;     with Scoop and portable installs, so deleting it would destroy the reading
;     positions of a syodep this installer never owned.
;   * SetErrorLevel before every Abort: NSIS exits 0 by default, so a silent
;     install can fail invisibly and any CI gate built on it is worthless.

Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef VERSION_NUMERIC
  !define VERSION_NUMERIC "0.0.0.0"
!endif
!ifndef SRCDIR
  !define SRCDIR "syodep-win64"
!endif
!ifndef LICENSE_FILE
  !define LICENSE_FILE "..\LICENSE"
!endif
; NSIS resolves a relative OutFile against the *script's* directory, not the
; working directory, so an unqualified name lands in packaging/. Always pass
; -DOUTFILE explicitly; the default only exists so a bare compile check works.
!ifndef OUTFILE
  !define OUTFILE "syodep-setup.exe"
!endif

; PASS EVERY PATH ABOVE AS AN ABSOLUTE PATH. The same script-relative rule
; applies to SRCDIR (via MUI_ICON and `File`) and to LICENSE_FILE, and it fails
; late and confusingly: a relative -DSRCDIR=syodep-win64 is hunted for at
; packaging/syodep-win64 and reported as "Error while loading icon ... can't
; open file", which reads like a missing icon rather than a wrong base
; directory.

!define APPNAME   "syodep"
!define PROGID    "syodep.pdf"
!define PUBLISHER "nexdep"
!define HOMEPAGE  "https://github.com/nexdep/syodep"
!define ARP_KEY   "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}"

Name "${APPNAME} ${VERSION}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\${APPNAME}"
InstallDirRegKey HKCU "Software\${APPNAME}" "InstallLocation"
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${VERSION_NUMERIC}"
VIAddVersionKey "ProductName"     "${APPNAME}"
VIAddVersionKey "ProductVersion"  "${VERSION}"
VIAddVersionKey "FileVersion"     "${VERSION}"
VIAddVersionKey "FileDescription" "${APPNAME} installer"
VIAddVersionKey "CompanyName"     "${PUBLISHER}"
VIAddVersionKey "LegalCopyright"  "AGPL-3.0-or-later"

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "Sections.nsh"
!include "LogicLib.nsh"

!define MUI_ABORTWARNING
!define MUI_ICON   "${SRCDIR}\syodep.ico"
!define MUI_UNICON "${SRCDIR}\syodep.ico"

; The AGPL grants rights; it does not require click-through assent. Show it for
; information rather than gating installation behind "I Agree".
!define MUI_LICENSEPAGE_TEXT_BOTTOM \
    "syodep is free software under the AGPL-3.0-or-later. Click Next to continue."
!define MUI_LICENSEPAGE_BUTTON "$(^NextBtn)"
!insertmacro MUI_PAGE_LICENSE "${LICENSE_FILE}"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\syodep.exe"
!define MUI_FINISHPAGE_NOAUTOCLOSE
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; ---------------------------------------------------------------------------
; Helpers
; ---------------------------------------------------------------------------

; `File` fails on a locked exe, and in silent mode it does so invisibly.
Function CheckNotRunning
    ${If} ${FileExists} "$INSTDIR\syodep.exe"
        ClearErrors
        Rename "$INSTDIR\syodep.exe" "$INSTDIR\syodep.exe.busycheck"
        ${If} ${Errors}
            SetErrorLevel 2
            Abort "syodep is running. Close it and run the installer again."
        ${EndIf}
        Rename "$INSTDIR\syodep.exe.busycheck" "$INSTDIR\syodep.exe"
    ${EndIf}
FunctionEnd

; A per-user installer pointed at, say, C:\Program Files via /D= would other-
; wise half-install and still report success.
; Checks the outcome rather than the error flag. CreateDirectory and FileOpen
; do not reliably raise it, so an earlier version of this function passed on a
; path it could not write to, the install failed later during File extraction,
; and the process still exited 0 -- a silent install that reported success
; having installed nothing.
Function CheckWritable
    CreateDirectory "$INSTDIR"
    ; "dir\*.*" is the NSIS idiom for "this directory exists".
    ${IfNot} ${FileExists} "$INSTDIR\*.*"
        SetErrorLevel 3
        Abort "Cannot create $INSTDIR."
    ${EndIf}
    ; A failed FileOpen leaves the handle empty, which is checkable directly.
    ClearErrors
    FileOpen $0 "$INSTDIR\.syodep-writetest" w
    ${If} $0 == ""
    ${OrIf} ${Errors}
        SetErrorLevel 4
        Abort "Cannot write to $INSTDIR."
    ${EndIf}
    FileWrite $0 "w"
    FileClose $0
    ; And confirm it truly landed, rather than trusting the write.
    ${IfNot} ${FileExists} "$INSTDIR\.syodep-writetest"
        SetErrorLevel 5
        Abort "Cannot write to $INSTDIR."
    ${EndIf}
    Delete "$INSTDIR\.syodep-writetest"
FunctionEnd

; `File /r` overwrites but never removes. A Qt6 DLL left behind from an older
; Qt next to newly installed ones is a startup crash with no useful message.
Function RemoveStalePayload
    ${If} ${FileExists} "$INSTDIR\syodep.exe"
        Delete "$INSTDIR\*.dll"
        RMDir /r "$INSTDIR\platforms"
        RMDir /r "$INSTDIR\styles"
        RMDir /r "$INSTDIR\imageformats"
        RMDir /r "$INSTDIR\iconengines"
        RMDir /r "$INSTDIR\generic"
        RMDir /r "$INSTDIR\networkinformation"
        RMDir /r "$INSTDIR\tls"
    ${EndIf}
FunctionEnd

; Makes syodep appear under "Open with" for PDFs. Purely additive: it claims
; nothing and overwrites no other application's keys.
Function RegisterOpenWith
    WriteRegStr HKCU "Software\Classes\${PROGID}" "" "PDF Document"
    WriteRegStr HKCU "Software\Classes\${PROGID}\DefaultIcon" "" "$INSTDIR\syodep.ico,0"
    WriteRegStr HKCU "Software\Classes\${PROGID}\shell\open" "FriendlyAppName" "${APPNAME}"
    WriteRegStr HKCU "Software\Classes\${PROGID}\shell\open\command" "" '"$INSTDIR\syodep.exe" "%1"'

    WriteRegStr HKCU "Software\Classes\Applications\syodep.exe" "FriendlyAppName" "${APPNAME}"
    WriteRegStr HKCU "Software\Classes\Applications\syodep.exe\shell\open\command" "" '"$INSTDIR\syodep.exe" "%1"'
    WriteRegStr HKCU "Software\Classes\Applications\syodep.exe\SupportedTypes" ".pdf" ""

    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
FunctionEnd

; Opt-in only. Note this CANNOT make syodep the default PDF handler: since
; Windows 8 the effective default lives in a hash-protected UserChoice key that
; an installer cannot forge. All this does is offer syodep in the picker and in
; Settings > Default apps, where the user confirms.
Function RegisterPdfAssociation
    WriteRegStr HKCU "Software\Classes\.pdf\OpenWithProgids" "${PROGID}" ""
    WriteRegStr HKCU "Software\${APPNAME}\Capabilities" "ApplicationName" "${APPNAME}"
    WriteRegStr HKCU "Software\${APPNAME}\Capabilities" "ApplicationDescription" \
        "Keyboard-first PDF reader"
    WriteRegStr HKCU "Software\${APPNAME}\Capabilities\FileAssociations" ".pdf" "${PROGID}"
    WriteRegStr HKCU "Software\RegisteredApplications" "${APPNAME}" \
        "Software\${APPNAME}\Capabilities"
    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
FunctionEnd

Function WriteArp
    ; EstimatedSize is a DWORD in KiB, and must be measured after the
    ; uninstaller exists or it undercounts.
    ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
    WriteRegStr   HKCU "${ARP_KEY}" "DisplayName"          "${APPNAME}"
    WriteRegStr   HKCU "${ARP_KEY}" "DisplayVersion"       "${VERSION}"
    WriteRegStr   HKCU "${ARP_KEY}" "DisplayIcon"          "$INSTDIR\syodep.exe,0"
    WriteRegStr   HKCU "${ARP_KEY}" "Publisher"            "${PUBLISHER}"
    WriteRegStr   HKCU "${ARP_KEY}" "InstallLocation"      "$INSTDIR"
    WriteRegStr   HKCU "${ARP_KEY}" "UninstallString"      '"$INSTDIR\Uninstall.exe"'
    WriteRegStr   HKCU "${ARP_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
    WriteRegStr   HKCU "${ARP_KEY}" "URLInfoAbout"         "${HOMEPAGE}"
    WriteRegStr   HKCU "${ARP_KEY}" "HelpLink"             "${HOMEPAGE}"
    WriteRegDWORD HKCU "${ARP_KEY}" "NoModify" 1
    WriteRegDWORD HKCU "${ARP_KEY}" "NoRepair" 1
    WriteRegDWORD HKCU "${ARP_KEY}" "EstimatedSize" "$0"
FunctionEnd

; ---------------------------------------------------------------------------
; Install
; ---------------------------------------------------------------------------

Section "syodep (required)" SecCore
    SectionIn RO
    SetShellVarContext current
    Call CheckWritable
    Call CheckNotRunning
    Call RemoveStalePayload

    SetOutPath "$INSTDIR"
    File /r "${SRCDIR}\*.*"

    Call RegisterOpenWith
    CreateShortcut "$SMPROGRAMS\${APPNAME}.lnk" "$INSTDIR\syodep.exe" "" \
        "$INSTDIR\syodep.ico" 0
    WriteRegStr HKCU "Software\${APPNAME}" "InstallLocation" "$INSTDIR"
    WriteUninstaller "$INSTDIR\Uninstall.exe"
    Call WriteArp
SectionEnd

; `/o` = unticked by default, in the wizard and under /S alike.
Section /o "Offer syodep as a PDF handler" SecAssoc
    Call RegisterPdfAssociation
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
    !insertmacro MUI_DESCRIPTION_TEXT ${SecCore} \
        "syodep and its Qt runtime."
    !insertmacro MUI_DESCRIPTION_TEXT ${SecAssoc} \
        "List syodep in Windows' PDF app picker. Windows still asks you to \
confirm the default in Settings."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; Defined after the sections: NSIS resolves ${SecAssoc} at parse time, so
; referencing it earlier silently expands to nothing.
Function .onInit
    SetShellVarContext current
    ; Silent mode takes section defaults, so /S alone must not associate.
    ; /ASSOCIATE opts in. GetOptions sets the error flag when the switch is
    ; absent, hence IfNot Errors.
    ${GetParameters} $R0
    ClearErrors
    ${GetOptions} $R0 "/ASSOCIATE" $R1
    ${IfNot} ${Errors}
        !insertmacro SelectSection ${SecAssoc}
    ${EndIf}

    ; Under /S there is no directory page, so $INSTDIR is already final and a
    ; bad /D= can be rejected here. This matters for the exit code: Abort in a
    ; *section* cancels the install but still leaves the process exiting 0,
    ; whereas Abort in .onInit quits outright and preserves SetErrorLevel --
    ; without which a failed silent install is indistinguishable from success.
    ${If} ${Silent}
        Call CheckWritable
    ${EndIf}
FunctionEnd

; ---------------------------------------------------------------------------
; Uninstall
; ---------------------------------------------------------------------------

Function un.onInit
    ; The uninstaller does not inherit the installer's shell-var context.
    SetShellVarContext current
FunctionEnd

Section "Uninstall"
    ; $INSTDIR is user-controllable via /D= and via InstallDirRegKey, so prove
    ; the directory is ours before recursively deleting it.
    StrLen $0 "$INSTDIR"
    ${If} $0 < 5
        DetailPrint "Refusing to remove suspicious path: $INSTDIR"
        SetErrorLevel 2
        Abort
    ${EndIf}
    ${IfNot} ${FileExists} "$INSTDIR\syodep.exe"
    ${AndIfNot} ${FileExists} "$INSTDIR\Uninstall.exe"
        DetailPrint "$INSTDIR is not a syodep installation."
        SetErrorLevel 2
        Abort
    ${EndIf}

    Delete "$SMPROGRAMS\${APPNAME}.lnk"

    DeleteRegKey   HKCU "Software\Classes\${PROGID}"
    DeleteRegKey   HKCU "Software\Classes\Applications\syodep.exe"
    ; Never DeleteRegKey the .pdf class itself -- it holds other applications'
    ; OpenWithProgids and OpenWithList entries.
    DeleteRegValue HKCU "Software\Classes\.pdf\OpenWithProgids" "${PROGID}"
    DeleteRegKey /ifempty HKCU "Software\Classes\.pdf\OpenWithProgids"
    DeleteRegValue HKCU "Software\RegisteredApplications" "${APPNAME}"
    DeleteRegKey   HKCU "Software\${APPNAME}"
    DeleteRegKey   HKCU "${ARP_KEY}"

    RMDir /r "$INSTDIR"

    ; %APPDATA%\syodep is deliberately left alone: config and reading positions
    ; are shared with Scoop and portable installs this uninstaller does not own.
    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
SectionEnd
