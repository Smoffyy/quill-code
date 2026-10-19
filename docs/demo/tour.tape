Output docs/demo/tour.gif

Set Shell "powershell"
Set FontSize 16
Set Width 1000
Set Height 620
Set Padding 20
Set Framerate 10
Set TypingSpeed 20ms
Set Theme { "name": "Quill", "background": "#1f1f1e", "foreground": "#e9e7e2", "cursor": "#e9e7e2", "selection": "#3a3936", "black": "#1f1f1e", "red": "#e5786d", "green": "#8fbf7f", "yellow": "#e0b56a", "blue": "#7193f5", "magenta": "#c79bd8", "cyan": "#6fc2c8", "white": "#d6d3cc", "brightBlack": "#7d7a73", "brightRed": "#f08a80", "brightGreen": "#a4d394", "brightYellow": "#ecc888", "brightBlue": "#8eabf8", "brightMagenta": "#d6b0e4", "brightCyan": "#8ad3d8", "brightWhite": "#f4f3ee" }

Hide
Type '& ./docs/demo/setup.ps1; Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue; $env:APPDATA = "$env:TEMP\quill-demo\appdata"; Set-Location "$env:TEMP\quill-demo\slugs"; Clear-Host'
Enter
Sleep 3s
Show

Type "quill"
Enter
Wait+Screen@60s /›/
Shift+Tab
Wait+Screen@10s /Ask first/
Type "Rename slugify to toSlug everywhere."
Enter
Wait+Screen@300s /Allow everything in this thread/
Sleep 1s
Screenshot docs/images/approval.png
Sleep 2s
Escape
Sleep 1s
Escape
Sleep 3s

Type "/model"
Sleep 500ms
Enter
Wait+Screen@20s /Pick the model that powers Quill/
Sleep 1s
Screenshot docs/images/models.png
Sleep 2s
Escape
Sleep 1s
Ctrl+D
Sleep 3s

Hide
Type "Clear-Host"
Enter
Sleep 1s
Show
Type 'quill exec --access auto "Run the tests and tell me which ones fail."'
Enter
Sleep 5s
Wait@300s />\s*$/
Sleep 1s
Screenshot docs/images/exec.png
Sleep 2s