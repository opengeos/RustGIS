# Adds a folder to, or removes it from, the current user's PATH. Run by the setup.exe installer
# and uninstaller: path.ps1 add|remove <folder>. The value is read without expanding
# %VARIABLES%, so the rest of the PATH is written back unchanged.
param([Parameter(Mandatory)][ValidateSet('add', 'remove')][string]$Action,
      [Parameter(Mandatory)][string]$Folder)
$ErrorActionPreference = 'Stop'
$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
try {
  $old = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
  $parts = @($old -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -ne $Folder.TrimEnd('\')) })
  if ($Action -eq 'add') { $parts += $Folder }
  $new = $parts -join ';'
  if ($new -ne $old) {
    $key.SetValue('Path', $new, [Microsoft.Win32.RegistryValueKind]::ExpandString)
  }
} finally {
  $key.Close()
}
