$demo = Join-Path $env:TEMP 'quill-demo'
$project = Join-Path $demo 'slugs'
$appdata = Join-Path $demo 'appdata'
if (Test-Path $demo) { Remove-Item $demo -Recurse -Force }
New-Item -ItemType Directory (Join-Path $appdata 'quill-code') -Force | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'project') $project -Recurse
git -C $project init -q -b main
git -C $project add .
git -C $project -c user.name=Quill -c user.email=quill@localhost commit -q -m 'Add slugify'
$settings = Get-Content (Join-Path $env:APPDATA 'quill-code\settings.json') -Raw | ConvertFrom-Json
$settings.general.trusted = @($demo)
[IO.File]::WriteAllText((Join-Path $appdata 'quill-code\settings.json'), ($settings | ConvertTo-Json -Depth 20))