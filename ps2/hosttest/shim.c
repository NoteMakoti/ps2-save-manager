/* Host implementation of the libmc subset used by mcio.c, backed by
 * directories ./slot1 and ./slot2, plus stubs for storage.c. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dirent.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <fnmatch.h>
#include "libmc.h"
#include "ps2ips.h"
#include "../src/common.h"
#include "../src/storage.h"

static int last;
static char root[2][256];
static int present[2] = {1, 0};

static void host(int port, const char *p, char *out) {
    if (!root[port][0]) snprintf(root[port], 256, "%s/slot%d", getenv("CARD_ROOT") ? getenv("CARD_ROOT") : ".", port + 1);
    snprintf(out, 512, "%s%s", root[port], p);
}
int mcInit(int t) { (void)t; if (getenv("SLOT2")) present[1] = 1; return 0; }
int mcSync(int m, int *c, int *r) { (void)m; (void)c; *r = last; return 1; }
int mcGetInfo(int port, int s, int *type, int *fr, int *fmt) { (void)s; *type = present[port] ? 2 : 0; *fr = 7000; *fmt = 1; last = 0; return 0; }
int mcOpen(int port, int s, const char *n, int mode) {
    (void)s; char p[512]; host(port, n, p);
    int fl = (mode & 3) == 1 ? O_RDONLY : (mode & 3) == 2 ? O_WRONLY : O_RDWR;
    if (mode & 0x200) fl |= O_CREAT | O_TRUNC;
    int fd = open(p, fl, 0644); last = fd < 0 ? sceMcResNoEntry : fd; return 0; }
int mcClose(int fd) { last = close(fd); return 0; }
int mcSeek(int fd, int o, int w) { last = lseek(fd, o, w); return 0; }
int mcRead(int fd, void *b, int n) { last = read(fd, b, n); return 0; }
int mcWrite(int fd, const void *b, int n) { last = write(fd, b, n); return 0; }
int mcMkDir(int port, int s, const char *n) { (void)s; char p[512]; host(port, n, p); last = mkdir(p, 0755) ? -4 : 0; return 0; }
int mcDelete(int port, int s, const char *n) { (void)s; char p[512]; host(port, n, p);
    struct stat st; if (stat(p, &st)) { last = sceMcResNoEntry; return 0; }
    last = S_ISDIR(st.st_mode) ? (rmdir(p) ? sceMcResNotEmpty : 0) : unlink(p); return 0; }
int mcFormat(int port, int s) { (void)port; (void)s; last = 0; return 0; }
int mcSetFileInfo(int port, int s, const char *n, const sceMcTblGetDir *i, unsigned f) { (void)port;(void)s;(void)n;(void)i;(void)f; last = 0; return 0; }
int mcGetDir(int port, int s, const char *pat, unsigned m, int max, sceMcTblGetDir *t) {
    (void)s; (void)m; char dir[512], p[512]; const char *slash = strrchr(pat, '/');
    char d[256]; snprintf(d, sizeof d, "%.*s", (int)(slash - pat), pat); host(port, d[0] ? d : "", dir);
    DIR *D = opendir(dir); if (!D) { last = sceMcResNoEntry; return 0; }
    int n = 0; struct dirent *e;
    memset(t, 0, sizeof(*t) * 2);
    strcpy((char*)t[n++].EntryName, "."); strcpy((char*)t[n++].EntryName, "..");
    while ((e = readdir(D)) && n < max) {
        if (e->d_name[0] == '.' || fnmatch(slash + 1, e->d_name, 0)) continue;
        struct stat st; snprintf(p, sizeof p, "%s/%s", dir, e->d_name); stat(p, &st);
        memset(&t[n], 0, sizeof t[n]);
        strncpy((char*)t[n].EntryName, e->d_name, 31);
        t[n].AttrFile = S_ISDIR(st.st_mode) ? 0x8427 : 0x8497;
        t[n].FileSizeByte = S_ISDIR(st.st_mode) ? 0 : st.st_size;
        t[n]._Modify.Year = t[n]._Create.Year = 2005; t[n]._Modify.Month = t[n]._Create.Month = 6; t[n]._Modify.Day = t[n]._Create.Day = 1;
        n++;
    }
    closedir(D); last = n; return 0; }

/* storage.c stand-ins */
int storage_available(target_t t) { (void)t; return 0; }
int mmce_get(int port, int *c, int *ch, char *g, int gl) { (void)port; *c = 3; *ch = 2; snprintf(g, gl, "SLUS-20312"); return 0; }
int mmce_set(int port, int what, int mode, int num) { (void)port; (void)what; (void)mode; return num; }

#include "../src/net.h"
int main(int argc, char **argv) {
    g_have_net = 1; strcpy(g_ip_str, "127.0.0.1");
    config_defaults(&g_cfg);
    int port = argc > 1 ? atoi(argv[1]) : 6524;
    mcInit(1);
    if (net_server_start(port)) { perror("listen"); return 1; }
    fprintf(stderr, "host test server on %d\n", port);
    for (;;) { net_server_poll(100); }
}
int ps2ipc_ps2ip_getconfig(char *n, t_ip_info *i) { (void)n; (void)i; return 0; }
int ps2ipc_ps2ip_setconfig(const t_ip_info *i) { (void)i; return 0; }
