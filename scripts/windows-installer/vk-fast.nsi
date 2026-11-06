Unicode true
RequestExecutionLevel user
ManifestSupportedOS Win10
SetCompressor /SOLID lzma
SetCompressorDictSize 32
CRCCheck on
SetFont "Segoe UI" 9

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "nsDialogs.nsh"
!include "x64.nsh"
!include "WinVer.nsh"

!define APP_KEY "Software\VK Fast"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\VKFast"
!define STORAGE_FILE "vk-fast-storage.ini"

Name "VK Fast"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\VK Fast"
InstallDirRegKey HKCU "${APP_KEY}" "InstallDir"
BrandingText "VK Fast"
Icon "${APP_ICON}"
UninstallIcon "${APP_ICON}"
VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "VK Fast"
VIAddVersionKey /LANG=1033 "FileDescription" "VK Fast Setup"
VIAddVersionKey /LANG=1033 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "VK Fast contributors"

!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
!define MUI_ABORTWARNING
!define MUI_LANGDLL_REGISTRY_ROOT HKCU
!define MUI_LANGDLL_REGISTRY_KEY "${APP_KEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "InstallerLanguage"
!define MUI_FINISHPAGE_RUN "$INSTDIR\vk-fast.exe"
!define MUI_FINISHPAGE_RUN_TEXT "$(RunApp)"
!define MUI_FINISHPAGE_RUN_NOTCHECKED
!define MUI_UNCONFIRMPAGE_TEXT_TOP "$(UninstallInfo)"

!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE DirectoryLeave
!insertmacro MUI_PAGE_DIRECTORY
Page custom StorageCreate StorageLeave
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Russian"

LangString RunApp ${LANG_ENGLISH} "Run VK Fast"
LangString RunApp ${LANG_RUSSIAN} "Запустить VK Fast"
LangString NeedWindows ${LANG_ENGLISH} "VK Fast requires Windows 10 or newer (64-bit)."
LangString NeedWindows ${LANG_RUSSIAN} "Для VK Fast нужна Windows 10 или новее (64-битная)."
LangString StorageTitle ${LANG_ENGLISH} "Settings and cache"
LangString StorageTitle ${LANG_RUSSIAN} "Настройки и кэш"
LangString StorageSubtitle ${LANG_ENGLISH} "Choose where VK Fast will store your data."
LangString StorageSubtitle ${LANG_RUSSIAN} "Выберите, где VK Fast будет хранить данные."
LangString ConfigLabel ${LANG_ENGLISH} "Settings, sign-in and history"
LangString ConfigLabel ${LANG_RUSSIAN} "Настройки, вход и история"
LangString CacheLabel ${LANG_ENGLISH} "Cache: audio, artwork and library"
LangString CacheLabel ${LANG_RUSSIAN} "Кэш: аудио, обложки и библиотека"
LangString AppDataChoice ${LANG_ENGLISH} "AppData (current Windows user)"
LangString AppDataChoice ${LANG_RUSSIAN} "AppData (текущий пользователь Windows)"
LangString PortableChoice ${LANG_ENGLISH} "Installation folder"
LangString PortableChoice ${LANG_RUSSIAN} "Папка установки"
LangString DesktopChoice ${LANG_ENGLISH} "Create a desktop shortcut"
LangString DesktopChoice ${LANG_RUSSIAN} "Создать ярлык на рабочем столе"
LangString StorageInfo ${LANG_ENGLISH} "Existing data in the selected folders is kept. Data from other folders is not moved automatically."
LangString StorageInfo ${LANG_RUSSIAN} "Данные в выбранных папках сохраняются. Данные из других папок автоматически не переносятся."
LangString WriteError ${LANG_ENGLISH} "Cannot write to:$\r$\n$0$\r$\nChoose a folder available to your Windows user."
LangString WriteError ${LANG_RUSSIAN} "Нет доступа для записи:$\r$\n$0$\r$\nВыберите папку, доступную вашему пользователю Windows."
LangString CloseApp ${LANG_ENGLISH} "Close VK Fast completely, including its tray icon, then click Retry."
LangString CloseApp ${LANG_RUSSIAN} "Полностью закройте VK Fast, включая значок в трее, и нажмите «Повторить»."
LangString InvalidChoice ${LANG_ENGLISH} "Invalid storage option. Use appdata or portable for /CONFIG= and /CACHE=."
LangString InvalidChoice ${LANG_RUSSIAN} "Неверный вариант хранения. Для /CONFIG= и /CACHE= используйте appdata или portable."
LangString SaveError ${LANG_ENGLISH} "Could not save the storage selection. Installation was not completed."
LangString SaveError ${LANG_RUSSIAN} "Не удалось сохранить расположение данных. Установка не завершена."
LangString UninstallInfo ${LANG_ENGLISH} "VK Fast will be removed. Your settings, sign-in, history and cache will be kept."
LangString UninstallInfo ${LANG_RUSSIAN} "VK Fast будет удалён. Настройки, вход, история и кэш сохранятся."

Var ConfigMode
Var CacheMode
Var ConfigPath
Var CachePath
Var ConfigArgument
Var CacheArgument
Var ConfigCombo
Var CacheCombo
Var ConfigPathLabel
Var CachePathLabel
Var DesktopCheck
Var CreateDesktop
Var StorageDialog
Var LoadedDirectory
Var PreviousDirectory
Var Parameters
Var StorageTemporary

Function .onInit
  SetRegView 64
  SetShellVarContext current
  ${Unless} ${Silent}
    !insertmacro MUI_LANGDLL_DISPLAY
  ${EndUnless}
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "$(NeedWindows)" /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "$(NeedWindows)" /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}
  ReadRegStr $PreviousDirectory HKCU "${APP_KEY}" "InstallDir"
  StrCpy $CreateDesktop ${BST_CHECKED}
  ${GetParameters} $Parameters
  ${GetOptions} $Parameters "/CONFIG=" $ConfigArgument
  ${GetOptions} $Parameters "/CACHE=" $CacheArgument
  ${If} $ConfigArgument != ""
  ${AndIf} $ConfigArgument != "appdata"
  ${AndIf} $ConfigArgument != "portable"
    Goto invalid_options
  ${EndIf}
  ${If} $CacheArgument != ""
  ${AndIf} $CacheArgument != "appdata"
  ${AndIf} $CacheArgument != "portable"
    Goto invalid_options
  ${EndIf}
  Return
  invalid_options:
    MessageBox MB_OK|MB_ICONSTOP "$(InvalidChoice)" /SD IDOK
    SetErrorLevel 2
    Abort
FunctionEnd

Function LoadSelection
  ${If} $LoadedDirectory == $INSTDIR
    Return
  ${EndIf}
  StrCpy $ConfigMode "appdata"
  StrCpy $CacheMode "appdata"
  ; Preserve an existing portable installation when it has no selection marker.
  IfFileExists "$INSTDIR\config\*.*" 0 +2
    StrCpy $ConfigMode "portable"
  IfFileExists "$INSTDIR\cache\*.*" 0 +2
    StrCpy $CacheMode "portable"
  ReadINIStr $0 "$INSTDIR\${STORAGE_FILE}" "storage" "config"
  ${If} $0 == "appdata"
  ${OrIf} $0 == "portable"
    StrCpy $ConfigMode $0
  ${EndIf}
  ReadINIStr $0 "$INSTDIR\${STORAGE_FILE}" "storage" "cache"
  ${If} $0 == "appdata"
  ${OrIf} $0 == "portable"
    StrCpy $CacheMode $0
  ${EndIf}
  ${If} $ConfigArgument != ""
    StrCpy $ConfigMode $ConfigArgument
  ${EndIf}
  ${If} $CacheArgument != ""
    StrCpy $CacheMode $CacheArgument
  ${EndIf}
  StrCpy $LoadedDirectory $INSTDIR
FunctionEnd

Function ResolvePaths
  StrCpy $ConfigPath "$LOCALAPPDATA\VK Fast\config"
  StrCpy $CachePath "$LOCALAPPDATA\VK Fast\cache"
  ${If} $ConfigMode == "portable"
    StrCpy $ConfigPath "$INSTDIR\config"
  ${EndIf}
  ${If} $CacheMode == "portable"
    StrCpy $CachePath "$INSTDIR\cache"
  ${EndIf}
FunctionEnd

Function RequireWritable
  Exch $0
  ClearErrors
  CreateDirectory "$0"
  IfErrors write_error
  GetTempFileName $1 "$0"
  IfErrors write_error
  Delete "$1"
  IfErrors write_error
  Pop $0
  Return
  write_error:
    MessageBox MB_OK|MB_ICONSTOP "$(WriteError)" /SD IDOK
    SetErrorLevel 3
    Abort
FunctionEnd

!macro RequireClosed PREFIX
Function ${PREFIX}RequireClosed
  Exch $0
  IfFileExists "$0\vk-fast.exe" 0 closed
  retry:
    System::Call 'kernel32::CreateFileW(w "$0\vk-fast.exe", i 0x40000000, i 0, p 0, i 3, i 0x80, p 0) p.r1'
    ${If} $1 == -1
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(CloseApp)" /SD IDCANCEL IDRETRY retry
      SetErrorLevel 4
      Abort
    ${EndIf}
    System::Call 'kernel32::CloseHandle(p r1)'
  closed:
    Pop $0
FunctionEnd
!macroend
!insertmacro RequireClosed ""
!insertmacro RequireClosed "un."

Function DirectoryLeave
  Push "$INSTDIR"
  Call RequireWritable
FunctionEnd

Function StorageChanged
  Pop $0
  Call ReadStorageControls
FunctionEnd

Function ReadStorageControls
  SendMessage $ConfigCombo ${CB_GETCURSEL} 0 0 $0
  StrCpy $ConfigMode "appdata"
  ${If} $0 == 1
    StrCpy $ConfigMode "portable"
  ${EndIf}
  SendMessage $CacheCombo ${CB_GETCURSEL} 0 0 $0
  StrCpy $CacheMode "appdata"
  ${If} $0 == 1
    StrCpy $CacheMode "portable"
  ${EndIf}
  Call ResolvePaths
  ${NSD_SetText} $ConfigPathLabel $ConfigPath
  ${NSD_SetText} $CachePathLabel $CachePath
FunctionEnd

Function StorageCreate
  Call LoadSelection
  !insertmacro MUI_HEADER_TEXT "$(StorageTitle)" "$(StorageSubtitle)"
  nsDialogs::Create 1018
  Pop $StorageDialog
  ${If} $StorageDialog == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 10u "$(ConfigLabel)"
  Pop $0
  ${NSD_CreateDropList} 0 12u 100% 44u ""
  Pop $ConfigCombo
  ${NSD_CB_AddString} $ConfigCombo "$(AppDataChoice)"
  ${NSD_CB_AddString} $ConfigCombo "$(PortableChoice)"
  SendMessage $ConfigCombo ${CB_SETCURSEL} 0 0
  ${If} $ConfigMode == "portable"
    SendMessage $ConfigCombo ${CB_SETCURSEL} 1 0
  ${EndIf}
  ${NSD_CreateLabel} 0 29u 100% 20u ""
  Pop $ConfigPathLabel
  ${NSD_CreateLabel} 0 52u 100% 10u "$(CacheLabel)"
  Pop $0
  ${NSD_CreateDropList} 0 64u 100% 44u ""
  Pop $CacheCombo
  ${NSD_CB_AddString} $CacheCombo "$(AppDataChoice)"
  ${NSD_CB_AddString} $CacheCombo "$(PortableChoice)"
  SendMessage $CacheCombo ${CB_SETCURSEL} 0 0
  ${If} $CacheMode == "portable"
    SendMessage $CacheCombo ${CB_SETCURSEL} 1 0
  ${EndIf}
  ${NSD_CreateLabel} 0 81u 100% 20u ""
  Pop $CachePathLabel
  ${NSD_CreateCheckbox} 0 105u 100% 12u "$(DesktopChoice)"
  Pop $DesktopCheck
  ${NSD_SetState} $DesktopCheck $CreateDesktop
  ${NSD_CreateLabel} 0 122u 100% 28u "$(StorageInfo)"
  Pop $0
  ${NSD_OnChange} $ConfigCombo StorageChanged
  ${NSD_OnChange} $CacheCombo StorageChanged
  Call ReadStorageControls
  nsDialogs::Show
FunctionEnd

Function StorageLeave
  Call ReadStorageControls
  ${NSD_GetState} $DesktopCheck $CreateDesktop
FunctionEnd

Section "VK Fast" MainSection
  Call LoadSelection
  Call ResolvePaths
  Push "$INSTDIR"
  Call RequireClosed
  ${If} $PreviousDirectory != ""
  ${AndIf} $PreviousDirectory != $INSTDIR
    Push "$PreviousDirectory"
    Call RequireClosed
  ${EndIf}
  Push "$INSTDIR"
  Call RequireWritable
  Push "$ConfigPath"
  Call RequireWritable
  Push "$CachePath"
  Call RequireWritable
  SetOutPath "$INSTDIR"
  SetOverwrite on
  File /oname=vk-fast.exe "${APP_EXE}"
  SetOutPath "$INSTDIR"
  ClearErrors
  GetTempFileName $StorageTemporary "$INSTDIR"
  IfErrors storage_failed
  FileOpen $0 "$StorageTemporary" w
  IfErrors storage_failed
  FileWrite $0 "[storage]$\r$\nversion=1$\r$\nconfig=$ConfigMode$\r$\ncache=$CacheMode$\r$\n"
  FileClose $0
  IfErrors storage_failed
  System::Call 'kernel32::MoveFileExW(w "$StorageTemporary", w "$INSTDIR\${STORAGE_FILE}", i 9) i.r0'
  ${If} $0 == 0
    Goto storage_failed
  ${EndIf}
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\VK Fast.lnk" "$INSTDIR\vk-fast.exe" "" "$INSTDIR\vk-fast.exe" 0
  ${If} $CreateDesktop == ${BST_CHECKED}
    CreateShortcut "$DESKTOP\VK Fast.lnk" "$INSTDIR\vk-fast.exe" "" "$INSTDIR\vk-fast.exe" 0
  ${EndIf}
  WriteRegStr HKCU "${APP_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "VK Fast"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "VK Fast contributors"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\vk-fast.exe,0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/eml3ss/vk-fast"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" ${APP_SIZE_KB}
  SetErrorLevel 0
  Goto storage_done
  storage_failed:
    Delete "$StorageTemporary"
    MessageBox MB_OK|MB_ICONSTOP "$(SaveError)" /SD IDOK
    SetErrorLevel 5
    Abort
  storage_done:
SectionEnd

Function un.onInit
  SetRegView 64
  SetShellVarContext current
  !insertmacro MUI_UNGETLANGUAGE
FunctionEnd

Section "Uninstall"
  Push "$INSTDIR"
  Call un.RequireClosed
  SetOutPath "$TEMP"
  Delete "$INSTDIR\vk-fast.exe"
  RMDir "$INSTDIR\bin"
  Delete "$INSTDIR\${STORAGE_FILE}"
  Delete "$INSTDIR\Uninstall.exe"
  ReadRegStr $0 HKCU "${APP_KEY}" "InstallDir"
  ${If} $0 == $INSTDIR
    Delete "$SMPROGRAMS\VK Fast.lnk"
    Delete "$DESKTOP\VK Fast.lnk"
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    DeleteRegKey HKCU "${APP_KEY}"
  ${EndIf}
  ; Remove only an empty installation folder. User config/cache are retained.
  RMDir "$INSTDIR"
  SetErrorLevel 0
SectionEnd
