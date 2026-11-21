# The session mailbox

An **opt-in** view for one particular way of working: several Claude Code sessions that pass
messages to each other through a shared folder. It's **off by default**, and most people won't
need it. Settings → **Extensions** → **Mailbox** switches it on and adds its page.

## What it's for

Claude Code sessions on the same operating system can message each other directly (Claude Code's
`SendMessage`). Sessions that run on different sides can't: for example one in Windows (a game or
app project) and one in WSL (the servers the app is deployed to). To let them work together
anyway, some people give them a **mailbox**: a plain folder that both sides can reach, where a
session leaves a Markdown message for another and picks up its own.

The mailbox works without Trayward: the sessions read and write the files themselves, following a
`README.md` in the mailbox folder. What's hard to see is the state of it all: is there mail nobody
has read, which session has to answer, has the message from this morning been handled? That's what
Trayward shows.

## How a mailbox looks

```text
C:\work\.claude-mailbox\           the mailbox (WSL sees it as /mnt/c/work/.claude-mailbox)
  README.md                       how the sessions use it
  ops\                            the inbox of the session working in the "ops" folder
    20261114T0557Z-shop-dns.md    unread: waiting to be handled
    done\                         handled messages
      20261113T1430Z-shop-ask.md
  shop\                           another session's inbox
    done\
```

- **An inbox per session**, named after the session's project folder.
- **A message** is a Markdown file named `<UTC time>-<sender's project>-<topic>.md`. It starts with
  `From:`, `To:`, `Date:` and `Re:` (the message it answers) lines.
- **Unread** messages lie directly in the inbox. A session that has handled one moves it to
  `done/`.
- A message being written is called `.<name>.tmp` until it's complete; Trayward leaves those out.

Trayward reads this layout. It doesn't set it up, and it never writes, moves or deletes a message:
the sessions do that.

## What Trayward shows

- **In the flyout,** a "Mailbox" section, once its folder is known:
  - **In its heading,** how many messages are unread, or "All handled".
  - **For each inbox with unread messages,** how many there are and how long the oldest has waited,
    with the newest one's sender and topic below. Point at it to see its unread messages by sender
    (each sender's newest topics and how long they've waited); click it to open the inbox in
    Explorer (a mailbox on a drive or in WSL; one on another PC's share isn't opened).
  - **When nothing is unread,** one line: how many inboxes there are and how many messages were
    handled in all. Click it to open the mailbox folder.
- **On a session's row,** `✉ 2` when that session's inbox (the one named after its project folder)
  has unread mail: it should look at its mail.
- **In the widget's tooltip,** a "Mailbox" row: what's unread and for whom (in yellow), or "All
  handled".
- **In Settings → Mailbox,** the folder in use (a green tick when it's found, a turning arrow while
  it's looked for, in yellow when it isn't there) and a table of the inboxes: unread (and how long
  the oldest has waited), handled, when the last one was handled, and whether a session runs in the
  project folder of that name.

The mailbox is read every 10 seconds while the option is on, and at once when it's switched on or
its folder changes. Nothing leaves the PC.

## Where the mailbox is

Trayward looks for a folder called `.claude-mailbox` in the folders above your sessions' working
folders (for a session in `C:\work\shop\api`, it tries `C:\work\shop\.claude-mailbox`, then
`C:\work\.claude-mailbox`, then `C:\.claude-mailbox`), and then in your user folder. The first one
found is used; the settings show which (`✓ Found: …`). Trayward remembers it (in its state file), so
it stays in use while no session runs below it, also after a restart; a session below another one
takes over. While it looks, the settings show a turning arrow; if it finds none, they say so and
wait for a folder to be pasted, and the flyout leaves the mailbox out.

If yours is somewhere else or has another name, copy its path (in Explorer: right-click the folder
→ **Copy as path**) and click **Paste** in Settings → Mailbox. **Find it** goes back to looking for
it.

## Settings

| Setting                              | Registry value (`HKCU\Software\Trayward`) | Default             |
| ------------------------------------ | ----------------------------------------- | ------------------- |
| Mailbox on or off                    | `Mailbox` (1: on)                         | off                 |
| The mailbox folder, when set by hand | `MailboxDir`                              | not set: looked for |

## For developers

- The code is in `core/src/mailbox.rs`: finding the folder (`find_root`), reading it (`read`) and the
  background thread that posts what it read to the app (`spawn`). The flyout section is
  `flyout::mailbox`; the session rows' `✉` comes from `App::mark_mail`.
- It follows the layout above and nothing more: a mailbox that's set up differently shows what
  matches (folders are inboxes, `*.md` files in them unread, `done/*.md` handled).
- The demo (`--demo`) has a made-up mailbox; switch the option on in the demo's settings to see it.
