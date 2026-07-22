# wtm — full-screen TUI demo (navigate, live status, filter, help)
# Requires: `wtm` on PATH and the demo fixture at ~/.wtm-demo (see README in this dir).
Output ../tui.gif

Set Shell bash
Set FontFamily "Menlo"
Set FontSize 18
Set Theme "Catppuccin Mocha"
Set Width 1440
Set Height 820
Set Padding 26
Set Margin 20
Set MarginFill "#0b0b0f"
Set BorderRadius 11
Set WindowBar Colorful
Set TypingSpeed 60ms

Hide
Type `export PS1='\W $ '`
Enter
Type "cd ~/.wtm-demo/acme"
Enter
Type `eval "$(command wtm init bash)"`
Enter
Type "clear"
Enter
Show

Sleep 700ms
Type "wtm tui"
Sleep 400ms
Enter
Sleep 3s
Down
Sleep 1s
Down
Sleep 1s
Down
Sleep 1.8s
Type "/"
Sleep 700ms
Type "feature"
Sleep 2s
Escape
Sleep 1.4s
Type "?"
Sleep 3.2s
Escape
Sleep 1s
Type "q"
Sleep 1.2s
