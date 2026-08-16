# bgapp2tray: Bridge between XDG Background Portal and system tray

bgapp2tray creates a tray icon for each background Flatpak app that uses the [XDG Background Portal](#ref1), allowing the user to activate or quit the app on systems not supporting the Background Portal (essentially every one except GNOME and KDE).

## Motivation

Probably due to GNOME not having a system tray (systray), some Flatpak apps use [XDG Background Portal](#ref1)) instead when running in background. (E.g., `GNOME Clock` and the music player `Amberol`.)

However, it seems that only GNOME and KDE support the Background Portal. In GNOME, Flatpak apps show in a menu in Quick Settings when running in background. In KDE, they appear in the system tray. In other desktop environments, these apps have no visible way to indicate that they are running in background, and the user has no way to interact with them, except launching the same app again.

Personally, I prefer systray to the Background Portal (some limitations of the latter explained later). But I do like some of those apps using the Background Portal (e.g., again, Amberol). So comes this project, which bridges between the two systems.

## References

1. <a name="ref1"></a>[XDG Background Portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Background.html)