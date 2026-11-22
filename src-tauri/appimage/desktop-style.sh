#! /usr/bin/env bash
# An AppRun hook of the AppImage (#143): release.mjs (addStyleHook) puts it
# in the AppImage's apprun-hooks folder, beside linuxdeploy-plugin-gtk.sh,
# and AppRun (under set -e) sources it right after that one, before the app
# starts.
#
# The GTK hook pins GTK_THEME to Adwaita:light or Adwaita:dark from the
# desktop's colour scheme (the portal's, else GNOME's setting), as custom
# themes are broken inside the bundle. Desktops whose dark look is a style
# have no such colour scheme: XFCE's Appearance sets the style (the GTK
# theme, Adwaita-dark), which the pin overrides, so the app started light
# there whatever the desktop. This hook reads the style too, once, at
# launch: a style whose name ends in -dark (or has -dark- in it) makes it
# Adwaita:dark. The pin stays. A switch while the app runs still needs a
# restart on such desktops (GNOME's colour scheme is followed live).
#
# Either order with the GTK hook works: before it, APPIMAGE_GTK_THEME is
# what the GTK hook takes; after it, GTK_THEME is set again. An
# APPIMAGE_GTK_THEME the user exported is left alone. POSIX sh only.

netterm_desktop_style() {
  # Without the bundle's GSettings schemas and GIO modules, which the GTK
  # hook may have pointed at already.
  case ":$(printf '%s' "${XDG_CURRENT_DESKTOP:-}" | tr '[:lower:]' '[:upper:]'):" in
    *:XFCE:*)
      env -u GSETTINGS_SCHEMA_DIR -u GIO_MODULE_DIR \
        timeout 2 xfconf-query -c xsettings -p /Net/ThemeName 2> /dev/null &&
        return
      ;;
  esac
  env -u GSETTINGS_SCHEMA_DIR -u GIO_MODULE_DIR \
    timeout 2 gsettings get org.gnome.desktop.interface gtk-theme 2> /dev/null
}

if ! printenv APPIMAGE_GTK_THEME > /dev/null 2>&1; then
  case "$(netterm_desktop_style | tr -d "'" | tr '[:upper:]' '[:lower:]')" in
    *-dark | *-dark-*)
      APPIMAGE_GTK_THEME="Adwaita:dark"
      export GTK_THEME="$APPIMAGE_GTK_THEME"
      ;;
  esac
fi
unset -f netterm_desktop_style
