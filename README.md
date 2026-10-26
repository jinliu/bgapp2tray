# bgapp2tray: Bridge between XDG Background Portal and system tray

bgapp2tray creates a tray icon for each background Flatpak app that uses the [XDG Background Portal](#ref-portal-app), allowing the user to activate or quit the app on systems not supporting the Background Portal (essentially every one except GNOME and KDE).

## Motivation

Probably due to GNOME not having a system tray (systray), some Flatpak apps use [XDG Background Portal](#ref-portal-app)) instead when running in background. (E.g., `GNOME Clock` and the music player `Amberol`.)

However, it seems that only GNOME and KDE support the Background Portal. In GNOME, Flatpak apps show in a menu in Quick Settings when running in background. In KDE, they appear in the system tray. In other desktop environments, these apps have no visible way to indicate that they are running in background, and the user has no way to interact with them, except launching the same app again.

Personally, I prefer systray to the Background Portal (some limitations of the latter explained later). But I do like some of those apps using the Background Portal (e.g., again, Amberol). So comes this project, which bridges between the two systems.

## Build

```bash
git clone https://github.com/jinliu/bgapp2tray.git
cd bgapp2tray
cargo build --release
```

You'll find the compiled binaries in the `target/release` directory:

* `bgapp2tray`
* `bgapp2tray-helper`
* `xdg-desktop-portal-bgapp`

## Installation

You need to set up two services:

* `xdg-desktop-portal-bgapp`, an [XDG Desktop Portal backend](#ref-portal-backend-general), to enable the Background Portal for Flatpak apps.
* `bgapp2tray`, the tray icon manager, to provide a system tray icon for background Flatpak apps if they don't have one.

(These two are not tightly coupled. They only talk through the [XDG Desktop Portal Background Apps Monitor](#ref-background-monitor) interface. In theory, you could replace one without affecting the other. But in most cases, you will want to run both. More details in the [Technical Details](#technical-details) section.)

1. Copy `xdg-desktop-portal-bgapp`, `bgapp2tray`, and `bgapp2tray-helper` to a directory in your `$PATH`.

2. Copy `xdg-desktop-portal` config files to your `$HOME`, for ease of modification:

```bash
cp -r /usr/share/xdg-desktop-portal $HOME/.local/share/
```

(For alternate locations, see [XDG Desktop Portal configuration](#ref-portals-conf).)

3. Install the backend description file:

```bash
cp config-example/bgapp.portal $HOME/.local/share/xdg-desktop-portal/portals/
```

4. Modify the portal config file for your compositor. For example, if you are using `niri`, then you need to modify `$HOME/.local/share/xdg-desktop-portal/niri-portals.conf` to add the following line:

```txt
org.freedesktop.impl.portal.Background=bgapp;
```

5. Arrange for `xdg-desktop-portal-bgapp` and `bgapp2tray` to autostart:
```bash
mkdir -p $HOME/.local/share/dbus-1/services
cp config-example/org.freedesktop.impl.portal.desktop.bgapp.service $HOME/.local/share/dbus-1/services/
mkdir -p $HOME/.config/systemd/user
cp config-example/xdg-desktop-portal-bgapp.service config-example/bgapp2tray.service $HOME/.config/systemd/user/
```

### Configuration

`xdg-desktop-portal-bgapp` uses the Wayland [Foreign Toplevel List v1](#ref-foreign-toplevel-list) protocol to get the list of all app windows. It's supported by Cosmic, Hyprland, Niri, and all wlroots-based compositors (e.g., Sway). (GNOME and KDE Plasma don't support the protocol. But their own portal backends use private protocols to talk with their compositors. So you don't need this project in this case.)

If your compositor does not support the protocol, but has a command to list the app ID of all open windows, you can use that as a fallback. Copy the config file:

```bash
cp config-example/xdg-desktop-portal-bgapp.toml $HOME/.config/
```

then edit the `update_cmdline=` line in `$HOME/.config/xdg-desktop-portal-bgapp.toml`. The file originally has an example command for Niri (though Niri doesn't require it, because it supports the protocol). You should replace it with the appropriate command for your compositor. The command should output the list of all app IDs of currently open windows, one per line.

You can test your command by running it in the terminal:

```bash
sh -c "<your_command_here>"
```

You can also modify `update_interval_ms=1000` to change how frequently the command is run.

`bgapp2tray` uses a configuration file located at `$HOME/.config/bgapp2tray.toml`. You can copy the example configuration file and edit it according to your needs:

```bash
cp config-example/bgapp2tray.toml $HOME/.config/
```

Then edit `$HOME/.config/bgapp2tray.toml` as needed. You can change `tray_icon_status=1` to `tray_icon_status=0` to change the tray icon status from ActiveStatus to PassiveStatus. Depending on your systray implementation, this may affect how the tray icon is displayed, or make them totally invisible.

### Restart Your Session

Now everything is set up. Log out and log in again to apply the changes.

## Usage

If everything is set up correctly, if you set a Flatpak app to run in the background (e.g., enable "Background Playback" in Amberol's menu), then close its window, a tray icon should appear (maybe after a few seconds) indicating that the app is still running in the background.

You can click on the tray icon to bring the app back to the foreground. Right-click on the tray icon would open a menu, where you can quit the app.

## FAQ

1. Why doesn't the tray icon appear immediately after closing the app window?

The [Background Apps Monitor](#ref-background-monitor) has a pretty long delay, probably by design. More details in the [Technical Details](#technical-details) section.

2. Why no icons for Native apps (e.g. GNOME Clock or Amberol installed via your distribution's package manager)?

This is a known limitation of the XDG Background Portal. More details in the [Technical Details](#technical-details) section.

## Troubleshooting

When things don't work as expected, let's first check which of the two services isn't functioning correctly:

1. Start a background app. E.g., enable "Background Playback" in Amberol's menu and then close its window. Run `flatpak ps` should list an instance of `io.bassi.Amberol`.
1. After a while (usually 20 seconds is enough), run

```bash
busctl --user get-property org.freedesktop.background.Monitor /org/freedesktop/background/monitor org.freedesktop.background.Monitor BackgroundApps
```

to check the list of background apps.

The output should look like:

```bash
aa{sv} 1 2 "app_id" s "io.bassi.Amberol" "instance" s "1090165318"
```

(The number at the end doesn't matter.)

If the output is empty, then `xdg-desktop-portal` might not be running correctly. Otherwise, it's probably `bgapp2tray`. In either case, you might want to kill the service, and then run it with increased log level:

```bash
pkill -f xdg-desktop-portal-bgapp
RUST_LOG=debug xdg-desktop-portal-bgapp
```

## Technical Details

### `xdg-desktop-portal-bgapp`, the Portal backend

`xdg-desktop-portal` is the service ("frontend") that provides the XDG Desktop Portal API to applications. It talks to desktop-environment-specific backends like `xdg-desktop-portal-gnome` or `xdg-desktop-portal-gtk`. For the Background Portal, the [backend](#ref-portal-backend) needs to provide these interfaces to the frontend:

1. GetAppState: Return a list of app ID for open windows. We implement this via [Wayland Foreign Toplevel List v1 protocol](#ref-foreign-toplevel-list), or a user-provided custom command for his compositor.

1. NotifyBackground: Notify the user that the app has moved to the background, and ask him to confirm. We implement this as always silently approving. [GNOME](#ref-src-gnome-background) does something similar. [KDE Plasma](#ref-src-kde-background) shows a notification.

1. EnableAutostart: This is deprecated. We implement it as a no-op.

The [Background Monitor](#ref-background-monitor) uses the app ID list provided by `GetAppState` to infer background apps, by fetching the list of running Flatpak instances, and then removing those in the open-window list. To prevent newly-launched apps from being incorrectly marked as background when their windows have not yet appeared, it repeats the check a few times before making a final determination. Thus the pretty long and random delay in the monitor results. See comments in [code](#ref-src-portal-background).

### `bgapp2tray`, the tray icon manager

`bgapp2tray` creates one tray icon for each app ID from Background Monitor. If the monitor reports multiple Flatpak instances for the same app ID, only one icon is created, because:

1. The usual case when multiple instances of the same background app, is between when the app starts a dedicated background instance, and then quits itself. The Background Monitor will report both instances only briefly, so creating multiple tray icons would be unnecessary and confusing.

1. Even if multiple icons were created for the same app ID, clicking on any of them would activate the same Flatpak instance, because we activate the app by the DBus name corresponding to the app ID, not using the Flatpak instance ID.

We don't create tray icons if the app already has one. (E.g., Flatpak KDE apps like Elisa.) This prevents duplicate icons. We do this by getting the DBus address of all tray icons from `org.kde.StatusNotifierWatcher`, then converting them to PIDs via `org.freedesktop.DBus.GetConnectionUnixProcessID`, and finally, getting the Flatpak instance IDs from `/proc/<pid>/root/.flatpak-info`. This is copied from the [KDE Plasma code](#ref-src-kde-ui).

One interesting implementation detail is that we don't create tray icons directly in `bgapp2tray`, but spawn a separate `bgapp2tray-helper` process for each icon. There are multiple reasons:

1. Althought the [libappindicator-zbus](#ref-libappindicator-zbus) library we use allows creating multiple tray icons in the same process, it doesn't support removing the tray icon once it is created, so we shut down the helper instead.

1. Similarly, some tray implementations (e.g., the one in [Noctalia](#ref-noctalia) which I'm using) also don't expect multiple tray icons under the same DBus name, so they only remove icons when the corresponding process exits.

In theory, you should be above to run `bgapp2tray` in GNOME, along with the `appindicator` GNOME extension, to show background apps in the tray, in addition to Quick Settings. However, for some unknown reason, the `org.freedesktop.DBus.GetConnectionUnixProcessID` DBus call fails in GNOME. So this doesn't work.

## References

1. <a name="ref-portal-app"></a>[XDG Background Portal API for app](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Background.html)
1. <a name="ref-portal-backend"></a>[XDG Background Portal API for backend](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.Background.html)
1. <a name="ref-portal-backend-general"></a>[XDG Background Portal backend developer document](https://flatpak.github.io/xdg-desktop-portal/docs/for-desktop-developers.html)
1. <a name="ref-portals-conf"></a>[XDG Desktop Portal configuration](https://flatpak.github.io/xdg-desktop-portal/docs/portals.conf.html)
1. <a name="ref-background-monitor"></a>[XDG Background Monitor](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.background.Monitor.html)
1. <a name="ref-foreign-toplevel-list"></a>[Wayland Foreign Toplevel List v1 protocol](https://wayland.app/protocols/ext-foreign-toplevel-list-v1#ext_foreign_toplevel_handle_v1)
1. <a name="ref-src-gnome-background"></a>[GNOME Background Portal code](https://github.com/GNOME/xdg-desktop-portal-gnome/blob/main/src/background.c)
1. <a name="ref-src-gnome-ui"></a>[GNOME Shell Background Apps code](https://gitlab.gnome.org/GNOME/gnome-shell/-/merge_requests/2624)
1. <a name="ref-src-kde-background"></a>[KDE Plasma Background Portal code](https://github.com/KDE/xdg-desktop-portal-kde/blob/master/src/background.cpp)
1. <a name="ref-src-kde-ui"></a>[KDE Plasma Background Apps tray icon code](https://invent.kde.org/plasma/plasma-workspace/-/merge_requests/5703/diffs?commit_id=d3ff62bd3ec91253d1f196b6236e492c01b0acb1)
1. <a name="ref-src-portal-background"></a>[XDG Desktop Portal Background code](https://github.com/flatpak/xdg-desktop-portal/blob/main/desktop-portal/background.c)
1. <a name="ref-libappindicator-zbus"></a>[libappindicator-zbus crate](https://crates.io/crates/libappindicator-zbus)
1. <a name="ref-noctalia"></a>[Noctalia - Wayland Shell](https://noctalia.dev)