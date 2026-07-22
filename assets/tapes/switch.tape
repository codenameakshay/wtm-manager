# wtm — switch demo (cd into a worktree via the shell wrapper)
# Requires: `wtm` on PATH and the demo fixture at ~/.wtm-demo (see README in this dir).
Output ../switch.gif

Set Shell bash
Set FontFamily "Menlo"
Set FontSize 18
Set Theme "Catppuccin Mocha"
Set Width 1440
Set Height 380
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

Sleep 700ms
Type "wtm switch feature/login"
Sleep 500ms
Enter
Sleep 1.4s
Type "pwd"
Sleep 300ms
Enter
Sleep 1.2s
Type "git log --oneline -3"
Sleep 300ms
Enter
Sleep 2.8s
