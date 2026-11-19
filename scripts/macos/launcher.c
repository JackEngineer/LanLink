// Keep the bundle entry point native so Developer ID can seal a hardened executable.
#include <mach-o/dyld.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    char executable[PATH_MAX];
    char resolved[PATH_MAX];
    uint32_t size = sizeof(executable);
    if (_NSGetExecutablePath(executable, &size) != 0 ||
        realpath(executable, resolved) == NULL) {
        perror("LANLink launcher path");
        return 1;
    }
    char *separator = strrchr(resolved, '/');
    if (separator == NULL) return 1;
    *separator = '\0'; // Contents/MacOS
    separator = strrchr(resolved, '/');
    if (separator == NULL) return 1;
    *separator = '\0'; // Contents
    char script[PATH_MAX];
    int length = snprintf(script, sizeof(script), "%s/Resources/launch.sh", resolved);
    if (length < 0 || (size_t)length >= sizeof(script)) return 1;
    execl("/bin/bash", "bash", script, (char *)NULL);
    perror("LANLink launch");
    return 1;
}
