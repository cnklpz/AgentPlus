//! Native "save as" dialog for a text file (the folder picker lives in `projects`).

use anyhow::Result;
use std::path::PathBuf;

/// Asks where to save a `.txt` file, owned by the AgentPlus window. None when cancelled.
#[cfg(windows)]
pub fn save_txt(owner: isize, title: &str, file_name: &str) -> Result<Option<PathBuf>> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, SIGDN_FILESYSPATH};

    const CANCELLED: u32 = 0x800704C7; // HRESULT_FROM_WIN32(ERROR_CANCELLED)
    let (filter_name, filter_spec) = (HSTRING::from(crate::i18n::l("Text file (*.txt)", "文本文件 (*.txt)")), HSTRING::from("*.txt"));
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        dlg.SetOptions(dlg.GetOptions()? | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM)?;
        dlg.SetTitle(&HSTRING::from(title))?;
        dlg.SetFileName(&HSTRING::from(file_name))?;
        dlg.SetDefaultExtension(&HSTRING::from("txt"))?;
        // The HSTRINGs outlive the call that reads these pointers.
        dlg.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: PCWSTR(filter_name.as_ptr()), pszSpec: PCWSTR(filter_spec.as_ptr()) }])?;
        if let Err(e) = dlg.Show(HWND(owner as _)) {
            return if e.code().0 as u32 == CANCELLED { Ok(None) } else { Err(e.into()) };
        }
        let p = dlg.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as _));
        Ok(Some(PathBuf::from(s?)))
    }
}

/// AppleScript's `choose file name`. None when cancelled.
#[cfg(target_os = "macos")]
pub fn save_txt(_owner: isize, title: &str, file_name: &str) -> Result<Option<PathBuf>> {
    // Arguments go in through `argv`, never pasted into the script.
    const SCRIPT: &str = "on run argv
  activate
  try
    return POSIX path of (choose file name with prompt (item 1 of argv) default name (item 2 of argv))
  on error number -128
    return \"\"
  end try
end run";
    let out = std::process::Command::new("osascript").args(["-e", SCRIPT, title, file_name]).output()?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!p.is_empty()).then(|| PathBuf::from(if p.to_lowercase().ends_with(".txt") { p } else { format!("{p}.txt") })))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn save_txt(_owner: isize, _title: &str, _file_name: &str) -> Result<Option<PathBuf>> {
    anyhow::bail!("{}", crate::i18n::l("Saving files isn't supported on this system. Copy the key instead", "当前系统不支持保存文件，请改用复制"))
}
