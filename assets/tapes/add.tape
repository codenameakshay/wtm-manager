# wtm — add demo (create a worktree + run setup automation)
# Requires: `wtm` on PATH and the demo fixture at ~/.wtm-demo (see README in this dir).
Output ../add.gif

Set Shell bash
Set FontFamily "Menlo"
Set FontSize 18
Set Theme "Catppuccin Mocha"
Set Width 1440
Set Height 560
Set Padding 26
Set Margin 20
Set MarginFill "#0b0b0f"
Set BorderRadius 11
Set WindowBar Colorful
Set TypingSpeed 55ms

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

Sleep 600ms
Type "wtm add feature/notifications"
Sleep 500ms
Enter
Sleep 3.5s
Type "wtm list"
Sleep 400ms
Enter
Sleep 3.5s
