/*
 * PaTorrent.app's main executable (CFBundleExecutable "PaTorrent"), built by
 * .github/scripts/macos_bundle.py. GLib and GTK find their data and modules through compiled-in
 * Homebrew paths, which a Mac without Homebrew does not have; this points them at the copies
 * inside the bundle and then becomes patorrent-gtk (exec: same process, so macOS still sees the
 * app). The libraries themselves need nothing: their references were rewritten to
 * @executable_path/../Frameworks.
 *
 * PATORRENT_LAUNCHER_DEBUG=1 prints what it sets.
 */
#include <errno.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static char contents[PATH_MAX]; /* .../PaTorrent.app/Contents */

static void set(const char *name, const char *relative) {
    char value[PATH_MAX];
    if (snprintf(value, sizeof value, "%s/%s", contents, relative) >= (int)sizeof value) {
        fprintf(stderr, "PaTorrent: the bundle's path is too long\n");
        exit(127);
    }
    setenv(name, value, 1);
    if (getenv("PATORRENT_LAUNCHER_DEBUG"))
        fprintf(stderr, "PaTorrent launcher: %s=%s\n", name, value);
}

int main(int argc, char **argv) {
    char raw[PATH_MAX], exe[PATH_MAX], gui[PATH_MAX];
    uint32_t size = sizeof raw;
    if (_NSGetExecutablePath(raw, &size) != 0 || realpath(raw, exe) == NULL) {
        fprintf(stderr, "PaTorrent: cannot find its own path\n");
        return 127;
    }
    char *slash = strrchr(exe, '/'); /* exe: Contents/MacOS/PaTorrent */
    if (!slash) return 127;
    *slash = '\0';
    snprintf(gui, sizeof gui, "%s/patorrent-gtk", exe);
    slash = strrchr(exe, '/'); /* exe: Contents/MacOS */
    if (!slash) return 127;
    *slash = '\0';
    snprintf(contents, sizeof contents, "%s", exe);

    /* icon themes (Adwaita, hicolor) and anything else under share/ */
    set("XDG_DATA_DIRS", "Resources/share");
    set("GSETTINGS_SCHEMA_DIR", "Resources/share/glib-2.0/schemas");
    set("GDK_PIXBUF_MODULE_FILE", "Resources/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache");
    set("GDK_PIXBUF_MODULEDIR", "Resources/lib/gdk-pixbuf-2.0/2.10.0/loaders");
    set("GIO_MODULE_DIR", "Resources/lib/gio/modules");
    /* GTK's own modules (print backends, media) would come from Homebrew's lib/gtk-4.0, which
       links a second GTK: point it at the bundle, which carries none */
    set("GTK_EXE_PREFIX", "Resources");
    set("GTK_DATA_PREFIX", "Resources");
    set("GTK_PATH", "Resources/lib/gtk-4.0");

    /* macOS before 10.9 passed a "-psn_..." process serial number, which the GUI would take
       for a file name */
    char **args = calloc((size_t)argc + 1, sizeof *args);
    if (!args) return 127;
    int n = 0;
    args[n++] = gui;
    for (int i = 1; i < argc; i++)
        if (strncmp(argv[i], "-psn_", 5) != 0) args[n++] = argv[i];
    args[n] = NULL;
    execv(gui, args);
    fprintf(stderr, "PaTorrent: cannot start %s: %s\n", gui, strerror(errno));
    return 127;
}
