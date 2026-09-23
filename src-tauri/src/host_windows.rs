//! Windows SSH bootstrap uses explicit PowerShell argv data, never POSIX quoting.
use crate::host_service::{HostStream, HostTarget};
use crate::ssh_service::SshManager;
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) fn is_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || path.starts_with(r"\\")
}

pub(crate) fn quote(value: &str) -> Result<String, String> {
    if value.is_empty() || value.contains('\0') {
        return Err("invalid-windows-argument".into());
    }
    Ok(format!("'{}'", value.replace('\'', "''")))
}

pub(crate) fn command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

pub(crate) fn metadata_script(
    binary: &str,
    session: &str,
    operation: &str,
) -> Result<String, String> {
    if !matches!(
        operation,
        "api schema --json" | "session list --json" | "status --json"
    ) {
        return Err("invalid-runtime-probe-operation".into());
    }
    Ok(format!(
        r#"
$info=New-Object Diagnostics.ProcessStartInfo
$info.FileName={binary}
$info.Arguments={operation}
$info.UseShellExecute=$false
$info.CreateNoWindow=$true
$info.RedirectStandardOutput=$true
$info.RedirectStandardError=$true
$info.EnvironmentVariables['HERDR_SESSION']={session}
$info.EnvironmentVariables.Remove('HERDR_SOCKET_PATH')
$info.EnvironmentVariables.Remove('HERDR_ENV')
$process=New-Object Diagnostics.Process
$process.StartInfo=$info
try {{
 $null=$process.Start()
 $errors=$process.StandardError.ReadToEndAsync()
 $process.StandardOutput.BaseStream.CopyTo([Console]::OpenStandardOutput())
 $process.WaitForExit()
 $null=$errors.GetAwaiter().GetResult()
 if ($process.ExitCode -ne 0) {{ throw 'herdr-command-failed' }}
}} finally {{ $process.Dispose() }}
"#,
        binary = quote(binary)?,
        session = quote(session)?,
        operation = quote(operation)?
    ))
}

pub(crate) fn helper_command(helper: &str, argument: &str) -> Result<String, String> {
    if !matches!(argument, "--stdio" | "--stream" | "--tcp" | "--database") {
        return Err("invalid-host-lane".into());
    }
    // PowerShell's native-command pipeline decodes/re-encodes stdout and can
    // buffer it by line. Forward the .NET byte streams instead (UTF-8 protocol).
    Ok(command(&format!(
        r#"
$ErrorActionPreference='Stop'
$info=New-Object Diagnostics.ProcessStartInfo
$info.FileName={helper}
$info.Arguments={argument}
$info.UseShellExecute=$false
$info.CreateNoWindow=$true
$info.RedirectStandardInput=$true
$info.RedirectStandardOutput=$true
$info.RedirectStandardError=$true
$process=New-Object Diagnostics.Process
$process.StartInfo=$info
try {{
 $null=$process.Start()
 $inputTask=[Console]::OpenStandardInput().CopyToAsync($process.StandardInput.BaseStream)
 $outputTask=$process.StandardOutput.BaseStream.CopyToAsync([Console]::OpenStandardOutput())
 $errorTask=$process.StandardError.BaseStream.CopyToAsync([Console]::OpenStandardError())
 $inputClosed=$false
 while (-not $process.WaitForExit(50)) {{
  if (-not $inputClosed -and $inputTask.IsCompleted) {{
   $inputTask.GetAwaiter().GetResult()
   $process.StandardInput.Close()
   $inputClosed=$true
  }}
 }}
 $outputTask.GetAwaiter().GetResult()
 $errorTask.GetAwaiter().GetResult()
 $result=$process.ExitCode
}} finally {{
 if ($process.Id -and -not $process.HasExited) {{ $process.Kill(); $process.WaitForExit() }}
 $process.Dispose()
}}
exit $result
"#,
        helper = quote(helper)?,
        argument = quote(argument)?
    )))
}

pub(crate) async fn execute(
    target: &HostTarget,
    ssh: Option<&SshManager>,
    script: &str,
    input: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, String> {
    let HostTarget::Ssh { session_id } = target else {
        return Err("windows-ssh-required".into());
    };
    let script = format!(
        r#"$ErrorActionPreference='Stop'
try {{
{script}
$marker=[Text.Encoding]::UTF8.GetBytes([string][char]0+'YUZORA_OK'+[char]0)
[Console]::OpenStandardOutput().Write($marker,0,$marker.Length)
}} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}"#
    );
    let mut stream: HostStream = ssh
        .ok_or("ssh-connection-required")?
        .open_host_exec(session_id, &command(&script))
        .await?;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        stream.write_all(input).await.map_err(|e| e.to_string())?;
        stream.shutdown().await.map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        (&mut stream)
            .take(max_output as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        if bytes.len() > max_output {
            return Err("host-probe-output-too-large".into());
        }
        let suffix = b"\0YUZORA_OK\0";
        if !bytes.ends_with(suffix) {
            return Err("windows-host-command-failed".into());
        }
        bytes.truncate(bytes.len() - suffix.len());
        Ok(bytes)
    })
    .await
    .map_err(|_| "host-setup-timeout".to_owned())?
}

pub(crate) const PROBE: &str = r#"
if (-not [Environment]::Is64BitOperatingSystem) { throw 'unsupported-runtime-architecture' }
$arch=[Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE')
if ($arch -ne 'AMD64') { throw 'unsupported-runtime-architecture' }
$homePath=[IO.Path]::GetFullPath($env:USERPROFILE)
$herdrPath=(Get-Command herdr.exe -ErrorAction SilentlyContinue).Source
$text='Windows'+[char]0+'x86_64'+[char]0+$homePath+[char]0+$herdrPath+[char]0
$bytes=[Text.Encoding]::UTF8.GetBytes($text)
[Console]::OpenStandardOutput().Write($bytes,0,$bytes.Length)
"#;

pub(crate) fn deploy_script(
    home: &str,
    version: &str,
    name: &str,
    digest: &str,
) -> Result<(String, String), String> {
    if !is_windows_path(home)
        || version.contains(['/', '\\'])
        || name
            .split('/')
            .any(|p| p == ".." || p == "." || p.is_empty())
        || name.contains('\\')
    {
        return Err("invalid-windows-runtime-path".into());
    }
    let directory = format!("{home}/.local/share/yuzora/runtimes/{version}");
    let destination = format!("{directory}/{name}");
    let mut parents = vec![home.to_owned()];
    let mut current = home.trim_end_matches(['/', '\\']).to_owned();
    for part in [".local", "share", "yuzora", "runtimes", version]
        .into_iter()
        .chain(
            name.rsplit_once('/')
                .map(|(parent, _)| parent)
                .into_iter()
                .flat_map(|p| p.split('/')),
        )
    {
        current.push('/');
        current.push_str(part);
        parents.push(current.clone());
    }
    let parents = parents
        .iter()
        .map(|p| quote(p))
        .collect::<Result<Vec<_>, _>>()?
        .join(",");
    let script = format!(
        r#"
foreach ($path in @({parents})) {{
 if (Test-Path -LiteralPath $path) {{
  $item=Get-Item -Force -LiteralPath $path
  if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or -not $item.PSIsContainer) {{ throw 'unsafe-runtime-directory' }}
 }} else {{ [IO.Directory]::CreateDirectory($path) | Out-Null }}
}}
$destination={destination}
$expected={digest}
if (Test-Path -LiteralPath $destination) {{
 $item=Get-Item -Force -LiteralPath $destination
 if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or $item.PSIsContainer) {{ throw 'unsafe-runtime-file' }}
 if ((Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash.ToLowerInvariant() -ne $expected) {{ throw 'runtime-hash-mismatch' }}
 [Console]::OpenStandardInput().CopyTo([IO.Stream]::Null)
}} else {{
 $scratch=$destination+'.upload-'+[Guid]::NewGuid().ToString('N')
 try {{
  $file=[IO.File]::Open($scratch,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
  try {{ [Console]::OpenStandardInput().CopyTo($file); $file.Flush() }} finally {{ $file.Dispose() }}
  if ((Get-FileHash -Algorithm SHA256 -LiteralPath $scratch).Hash.ToLowerInvariant() -ne $expected) {{ throw 'runtime-hash-mismatch' }}
  [IO.File]::Move($scratch,$destination)
 }} finally {{ if (Test-Path -LiteralPath $scratch) {{ [IO.File]::Delete($scratch) }} }}
}}
"#,
        destination = quote(&destination)?,
        digest = quote(digest)?
    );
    Ok((destination, script))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoded_helper_keeps_unicode_and_metacharacters_as_literal_data() {
        let path = r"C:\Users\O'Brien\中文 $x & !\yuzora-host.exe";
        let cmd = helper_command(path, "--stdio").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(cmd.split_whitespace().last().unwrap())
            .unwrap();
        let script = String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(script.contains("O''Brien"));
        assert!(script.contains("中文 $x & !"));
        assert!(script.contains("'--stdio'"));
        assert!(!cmd.contains("$x"));
    }
    #[test]
    fn deployment_refuses_traversal_and_checks_reparse_points_and_hashes() {
        assert!(deploy_script(r"C:\Users\me", "../escape", "herdr.exe", "hash").is_err());
        assert!(deploy_script(r"C:\Users\me", "v1", "../herdr.exe", "hash").is_err());
        let (_, script) =
            deploy_script(r"C:\Users\me", "v1", "conpty/OpenConsole.exe", "abc").unwrap();
        for expected in [
            "ReparsePoint",
            "CreateNew",
            "Get-FileHash",
            "[IO.File]::Move",
            "conpty",
        ] {
            assert!(script.contains(expected));
        }
    }
}
