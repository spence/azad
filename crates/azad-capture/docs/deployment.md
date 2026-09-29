# Deploying device-level capture on a Mac

The capture helper replaces Azad's former event tap. Until its one-time setup is done, Azad has
no global shortcuts on that Mac, so set it up in this order, at the Mac, in one sitting.

## Setup

1. Install the virtual keyboard driver (skip if Karabiner-Elements is installed):

   ```sh
   cd crates/azad && just install-capture-driver
   ```

   Enter the administrator password, then allow the driver in System Settings -> General ->
   Login Items & Extensions -> Driver Extensions. `systemextensionsctl list | grep pqrs` shows
   `activated enabled` when done.

2. Install and start Azad:

   ```sh
   just install && just restart
   ```

   Azad registers its helper and opens Login Items & Extensions: allow "Azad" under App
   Background Activity. Then answer the "Azad Capture would like to receive keystrokes" prompt
   with Open System Settings and enable "Azad Capture" under Input Monitoring. The helper picks
   up the grant by itself within a few seconds.

3. Check:

   ```sh
   just status
   ```

   The helper's status line should read `"capture":"capturing"` with your keyboards `seized`.

4. Run the physical keyboard session:

   ```sh
   python3 crates/azad-capture/tests/physical/session.py \
     --out crates/azad-capture/docs/evidence/$(date -u +%F)-physical
   ```

## If typing stops

Quit Azad from its menu bar item with the mouse. The helper captures only while Azad is
connected, so it releases every keyboard at once and typing goes straight to macOS again.

## Updates

`just install` replaces the helper inside `Azad.app`; the running helper notices, exits, and
launchd starts the new one. The Input Monitoring grant is tied to the helper's identity
(`ai.azad.capture`, Developer ID team), so updates signed the same way keep it. Keyboards are
released while the helper restarts, so typing is never blocked by an update.

## Rolling back

Installing a build without the helper leaves the registered LaunchDaemon pointing at a program
that no longer exists, so no helper runs and no keyboard is seized. To remove the registration, use
System Settings -> General -> Login Items & Extensions, or run `sfltool resetbtm` (resets all
background item approvals). The driver can stay installed; it does nothing without a client.
