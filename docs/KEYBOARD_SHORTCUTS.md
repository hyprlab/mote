# Keyboard shortcuts

Press **Ctrl+?** (or *Main Menu → Keyboard Shortcuts*) for this list in
the app.

| Connections | |
| --- | --- |
| <kbd>Ctrl</kbd>+<kbd>N</kbd> | New connection |
| <kbd>Ctrl</kbd>+<kbd>Enter</kbd> | Connect to the selected connection |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | Edit the selected connection |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Search |
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | Import `.rdp` and `.vnc` files |

| General | |
| --- | --- |
| <kbd>Ctrl</kbd>+<kbd>,</kbd> | Settings |
| <kbd>Ctrl</kbd>+<kbd>?</kbd> | Keyboard shortcuts |
| <kbd>Ctrl</kbd>+<kbd>W</kbd> | Close the window |
| <kbd>Ctrl</kbd>+<kbd>Q</kbd> | Quit |

## In a session

While the remote desktop has the keyboard, every key goes to it, the
shortcuts above included, and so do the desktop's own (Super, Alt+Tab and
the rest) unless **Settings → Sessions → Grab Keyboard** is off. The desktop
takes the keyboard when it appears and whenever the pointer comes onto it,
and gives it back to this computer when the pointer leaves it. A drag that
leaves the desktop keeps the keyboard until its button is let go.

| Key | |
| --- | --- |
| <kbd>Right Ctrl</kbd>, alone | Let go of the keyboard: it moves to the session's menu, and the desktop's shortcuts work again. In fullscreen, the header shows until a click on the remote desktop takes the keyboard back. With another key, Right Ctrl goes to the remote desktop as usual. |
| <kbd>Super</kbd>+<kbd>Esc</kbd> | GNOME's own: gives the desktop's shortcuts back, leaving the other keys with the session. |

Keys the desktop would keep even then go from the session menu's
**Send Keys**: Ctrl+Alt+Delete, Super, Alt+Tab, Print Screen and Ctrl+Alt+F1
to F12.

## On macOS

The shortcuts above take <kbd>Command</kbd> in place of <kbd>Ctrl</kbd>, and
About, Settings and Quit are in the Mote menu. In a session, Command is the
remote desktop's Super and Option its Alt, and <kbd>Right Command</kbd>,
alone, lets go of the keyboard, as a MacBook has no Right Ctrl.

With **Grab Keyboard** on, macOS's own shortcuts go to the remote desktop
too: Command-Tab, Command-Space, Control and the arrows, and the Mote menu's
Command-Q and Command-H. That needs the Accessibility permission, which macOS
asks for the first time a session takes the keyboard (System Settings >
Privacy & Security > Accessibility); without it, the session gets every key
but those.
