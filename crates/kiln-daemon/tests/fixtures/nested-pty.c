/* Isolated Kiro-shaped PTY bridge for foreground identity regressions. */
#include <util.h>
#include <unistd.h>
#include <signal.h>
#include <sys/wait.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
static volatile sig_atomic_t stopping;
static void stop(int sig) { (void)sig; stopping = 1; }
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    int master;
    pid_t shell = forkpty(&master, NULL, NULL, NULL);
    if (shell < 0) return 3;
    if (!shell) { execl("/bin/bash", "bash", "--noprofile", "--norc", "-i", NULL); _exit(4); }
    signal(SIGTERM, stop);
    fcntl(master, F_SETFL, O_NONBLOCK);
    char path[4096], current[8192], last[8192] = "", output[8192];
    snprintf(path, sizeof(path), "%s/command", argv[1]);
    while (!stopping) {
        FILE *f = fopen(path, "r");
        if (f) {
            size_t n = fread(current, 1, sizeof(current)-2, f); fclose(f); current[n] = 0;
            if (n && strcmp(current, last)) {
                strcpy(last, current);
                if (!strcmp(current, "stop")) {
                    pid_t group = tcgetpgrp(master);
                    if (group > 0) kill(-group, SIGINT);
                } else { current[n++] = '\n'; write(master, current, n); }
            }
        }
        read(master, output, sizeof(output));
        usleep(20000);
    }
    pid_t group = tcgetpgrp(master);
    if (group > 0) kill(-group, SIGHUP);
    kill(-shell, SIGHUP);
    snprintf(path, sizeof(path), "%s/background", argv[1]);
    FILE *f = fopen(path, "r");
    if (f) { long background = 0; if (fscanf(f, "%ld", &background) == 1 && background > 0) kill(-(pid_t)background, SIGHUP); fclose(f); }
    close(master); waitpid(shell, NULL, 0);
    return 0;
}
