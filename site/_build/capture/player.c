#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
  if (argc < 2) return 2;
  FILE *f = fopen(argv[1], "r"); if (!f) return 1;
  char line[4096];
  while (fgets(line, sizeof line, f)) {
    if (!strncmp(line, "!activity ", 10)) { line[strcspn(line, "\n")] = 0; char cmd[256]; snprintf(cmd, sizeof cmd, "kiln activity %s", line + 10); fflush(stdout); system(cmd); continue; }
    if (!strncmp(line, "!title ", 7)) { line[strcspn(line, "\n")] = 0; printf("\033]0;%s\007", line + 7); fflush(stdout); continue; }
    if (!strncmp(line, "!clear", 6)) { printf("\033[H\033[2J"); fflush(stdout); continue; }
    if (!strncmp(line, "!read", 5)) { fflush(stdout); char b[256]; fgets(b, sizeof b, stdin); continue; }
    if (!strncmp(line, "!sleep ", 7)) { fflush(stdout); usleep((useconds_t)(atof(line + 7) * 1e6)); continue; }
    if (!strncmp(line, "!type ", 6)) { line[strcspn(line, "\n")] = 0; for (char *c = line + 6; *c; c++) { putchar(*c); fflush(stdout); if ((*c & 0xC0) != 0x80) usleep(38000); } putchar('\n'); fflush(stdout); continue; }
    if (!strncmp(line, "!hold", 5)) { fflush(stdout); for (;;) pause(); }
    fputs(line, stdout); fflush(stdout); usleep(120000);
  }
  return 0;
}
