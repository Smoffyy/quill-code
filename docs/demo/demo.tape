Output docs/images/demo.gif

Set Shell "powershell"
Set FontSize 16
Set Width 1200
Set Height 720
Set Padding 20
Set Framerate 20
Set TypingSpeed 45ms
Set Theme { "name": "Quill", "background": "#1f1f1e", "foreground": "#e9e7e2", "cursor": "#e9e7e2", "selection": "#3a3936", "black": "#1f1f1e", "red": "#e5786d", "green": "#8fbf7f", "yellow": "#e0b56a", "blue": "#7193f5", "magenta": "#c79bd8", "cyan": "#6fc2c8", "white": "#d6d3cc", "brightBlack": "#7d7a73", "brightRed": "#f08a80", "brightGreen": "#a4d394", "brightYellow": "#ecc888", "brightBlue": "#8eabf8", "brightMagenta": "#d6b0e4", "brightCyan": "#8ad3d8", "brightWhite": "#f4f3ee" }

Hide
Type '& ./docs/demo/setup.ps1; Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue; $env:APPDATA = "$env:TEMP\quill-demo\appdata"; Set-Location "$env:TEMP\quill-demo\slugs"; Clear-Host'
Enter
Sleep 3s
Show

Type "quill"
Enter
Wait+Screen@60s /›/
Sleep 1.5s
Type "npm test fails. Find out why and fix it."
Sleep 500ms
Enter
Wait+Screen@600s /Worked for/
Sleep 5s