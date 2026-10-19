& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $repo = 'Smoffyy/quill-code'
    $version = if ($env:QUILL_VERSION) { $env:QUILL_VERSION } else { 'latest' }
    $dir = if ($env:QUILL_INSTALL_DIR) { $env:QUILL_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\quill' }
    $target = 'x86_64-pc-windows-msvc'
    $archive = "quill-$target.zip"

    if ($env:QUILL_DOWNLOAD_URL) {
        $base = $env:QUILL_DOWNLOAD_URL
    } elseif ($version -eq 'latest') {
        $base = "https://github.com/$repo/releases/latest/download"
    } else {
        $base = "https://github.com/$repo/releases/download/v$($version.TrimStart('v'))"
    }

    function Get-QuillFile($name, $path) {
        $source = "$base/$name"
        if ($source -match '^https?://') {
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -Uri $source -OutFile $path -UseBasicParsing
        } else {
            Copy-Item -LiteralPath (Join-Path $base $name) -Destination $path
        }
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) ("quill-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        Write-Host "Downloading quill for $target..."
        Get-QuillFile $archive (Join-Path $work $archive)
        Get-QuillFile 'SHA256SUMS' (Join-Path $work 'SHA256SUMS')

        $line = Get-Content (Join-Path $work 'SHA256SUMS') | Where-Object { $_ -match "\s\*?$([regex]::Escape($archive))$" } | Select-Object -First 1
        if (-not $line) { throw "The release has no checksum for $archive." }
        $expected = ($line -split '\s+')[0]
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $work $archive)).Hash
        if ($expected -ne $actual) { throw 'The download is damaged (checksum mismatch). Try again.' }

        Expand-Archive -LiteralPath (Join-Path $work $archive) -DestinationPath (Join-Path $work 'files') -Force
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $exe = Join-Path $dir 'quill.exe'
        $old = "$exe.old"
        if (Test-Path -LiteralPath $old) { Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }
        if (Test-Path -LiteralPath $exe) { Move-Item -LiteralPath $exe -Destination $old -Force }
        Copy-Item -LiteralPath (Join-Path $work 'files\quill.exe') -Destination $exe

        $installed = & $exe --version
        if ($LASTEXITCODE -ne 0) { throw "quill was copied to $dir but does not start." }
        Write-Host "Installed $installed to $exe"

        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $parts = @($userPath -split ';' | Where-Object { $_ })
        $onPath = ($env:Path -split ';') -contains $dir
        if ($onPath -or $parts -contains $dir) {
            if (-not $onPath) { $env:Path = "$dir;$env:Path" }
            Write-Host 'Run quill in any project folder to start.'
        } elseif ($env:QUILL_NO_MODIFY_PATH) {
            Write-Host "Add $dir to your PATH to run quill from anywhere."
        } else {
            [Environment]::SetEnvironmentVariable('Path', (@($dir) + $parts) -join ';', 'User')
            $env:Path = "$dir;$env:Path"
            Write-Host "Added $dir to your PATH. Run quill in any project folder to start."
        }
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}