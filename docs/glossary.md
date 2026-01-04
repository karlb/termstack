# TermStack Glossary

## Application

| Term | Meaning |
|------|---------|
| **TermStack** | A Wayland compositor that unifies terminal and graphical applications in a scrollable vertical column. Command outputs are separated into individual windows, allowing independent scrolling and closing while maintaining a traditional terminal workflow. |

## Core Concepts

| Term | Meaning |
|------|---------|
| **Stack** | The vertical column containing all windows. Windows are arranged top-to-bottom in the order they were created. |
| **Window** | Any container in the stack, whether terminal-based or graphical. |
| **Launcher terminal** | The initially present terminal window where commands are typed to launch new windows. Unlike other windows, it has no title bar. It starts with focus, contains the only shell prompt, and is the primary input point. Also called "launcher" for short. |
| **Terminal window** | A window displaying terminal/shell output from a command. |
| **GUI window** | A window containing a graphical application (Wayland or X11 via XWayland). |

## Commands

| Term | Meaning |
|------|---------|
| **Running command** | A command that is still executing. Its terminal window may still receive output. |
| **Finished command** | A command that has completed execution. Its terminal window remains visible for reference. |

## Navigation

| Term | Meaning |
|------|---------|
| **Scrolling the stack** | Moving the viewport up or down through all windows in the stack. |
| **Scrolling the window** | Scrolling the content within a single window (e.g., terminal scrollback or application content). |
| **Focus** | The currently active window that receives keyboard input. |
