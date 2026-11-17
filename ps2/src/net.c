/*
 * Network bring-up and the PS2SM protocol server (docs/PROTOCOL.md).
 *
 * The TCP/IP stack runs on the IOP (ps2ip-nm.irx + ps2ips.irx); the EE
 * uses BSD sockets through libps2ips/libcglue. Running the stack on the
 * IOP lets smbman.irx share it for the SMB backup target.
 */
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <kernel.h>
#include <delaythread.h>
#include <unistd.h>
#include <ps2ips.h>
#include <sys/socket.h>
#include <sys/select.h>
#include <netinet/in.h>
#include <arpa/inet.h>

#include "common.h"
#include "mcio.h"
#include "storage.h"
#include "net.h"

#ifndef DHCP_STATE_BOUND
#define DHCP_STATE_BOUND 10
#endif

/* t_ip_info comes in through ps2ips.h -> sys/socket.h */

/* exported by libps2ips (declared in its private ps2ipc.h) */
extern int ps2ipc_ps2ip_setconfig(const t_ip_info *ip_info);
extern int ps2ipc_ps2ip_getconfig(char *netif_name, t_ip_info *ip_info);

char g_net_status[96] = "stopped";
char g_net_last[96] = "";
int g_net_requests = 0;

static int listen_fd = -1;
static int client_fd = -1;

#define MAX_PAYLOAD (16 * 1024 * 1024)

enum {
    CMD_HELLO = 0x01,
    CMD_CARD_INFO,
    CMD_LIST_DIR,
    CMD_READ_FILE,
    CMD_WRITE_FILE,
    CMD_MKDIR,
    CMD_DELETE,
    CMD_GET_SAVE,
    CMD_PUT_SAVE,
    CMD_DELETE_SAVE,
    CMD_FORMAT,
    CMD_LIST_SAVES,
    CMD_MMCE_STATUS,
    CMD_MMCE_SET,
    CMD_BYE,
};

static void delay_ms(int ms)
{
    DelayThread(ms * 1000);
}

static void ip_to_str(struct in_addr a, char *out)
{
    u32 v = a.s_addr;
    sprintf(out, "%u.%u.%u.%u", v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, v >> 24);
}

int net_init(void (*progress)(const char *msg))
{
    if (ps2ip_init() < 0) {
        progress("ps2ips RPC init failed");
        return -1;
    }
    t_ip_info info;
    memset(&info, 0, sizeof(info));
    if (ps2ipc_ps2ip_getconfig("sm0", &info) < 0) {
        progress("no network adapter (sm0)");
        return -1;
    }
    if (g_cfg.use_dhcp) {
        progress("requesting DHCP lease...");
        info.dhcp_enabled = 1;
        ps2ipc_ps2ip_setconfig(&info);
        for (int i = 0; i < 150; i++) { /* ~15 s */
            delay_ms(100);
            if (ps2ipc_ps2ip_getconfig("sm0", &info) >= 0 && info.dhcp_status == DHCP_STATE_BOUND &&
                info.ipaddr.s_addr != 0)
                break;
        }
        if (info.dhcp_status != DHCP_STATE_BOUND) {
            progress("DHCP failed (cable? router?)");
            return -1;
        }
    } else {
        info.dhcp_enabled = 0;
        inet_aton(g_cfg.ip, &info.ipaddr);
        inet_aton(g_cfg.netmask, &info.netmask);
        inet_aton(g_cfg.gateway, &info.gw);
        ps2ipc_ps2ip_setconfig(&info);
        ps2ipc_ps2ip_getconfig("sm0", &info);
    }
    ip_to_str(info.ipaddr, g_ip_str);
    g_have_net = 1;
    return 0;
}

/* ------------------------------------------------------------------ */

/*
 * Socket I/O goes through a 64-byte aligned bounce buffer in pieces of at
 * most NET_CHUNK bytes. ps2ips.irx moves at most 1024 bytes per RPC through
 * a 1056-byte IOP buffer at offset 64, so a full 1024-byte transfer runs
 * past its end; staying at 960 bytes keeps every transfer inside it.
 */
#define NET_CHUNK 960
#define NET_IO_TIMEOUT_MS 15000 /* give up on a client that stops mid-message */

static u8 net_buf[NET_CHUNK] __attribute__((aligned(64)));

/* Wait until fd is readable (want_write=0) or writable. 1 = ready. */
static int wait_fd(int fd, int want_write, int timeout_ms)
{
    fd_set fs;
    FD_ZERO(&fs);
    FD_SET(fd, &fs);
    struct timeval tv = {timeout_ms / 1000, (timeout_ms % 1000) * 1000};
    int r = want_write ? select(fd + 1, NULL, &fs, NULL, &tv) : select(fd + 1, &fs, NULL, NULL, &tv);
    return r > 0 && FD_ISSET(fd, &fs);
}

static int send_all(int fd, const void *p, int n)
{
    const u8 *b = p;
    while (n > 0) {
        int c = n > NET_CHUNK ? NET_CHUNK : n;
        memcpy(net_buf, b, c);
        int done = 0;
        while (done < c) {
            if (!wait_fd(fd, 1, NET_IO_TIMEOUT_MS))
                return -1;
            int r = send(fd, net_buf + done, c - done, 0);
            if (r <= 0)
                return -1;
            done += r;
        }
        b += c;
        n -= c;
    }
    return 0;
}

static int recv_all(int fd, void *p, int n)
{
    u8 *b = p;
    while (n > 0) {
        if (!wait_fd(fd, 0, NET_IO_TIMEOUT_MS))
            return -1;
        int r = recv(fd, net_buf, n > NET_CHUNK ? NET_CHUNK : n, 0);
        if (r <= 0)
            return -1;
        memcpy(b, net_buf, r);
        b += r;
        n -= r;
    }
    return 0;
}

static u16 rd16(const u8 *p) { return p[0] | (p[1] << 8); }
static u32 rd32(const u8 *p) { return p[0] | (p[1] << 8) | (p[2] << 16) | ((u32)p[3] << 24); }

/* payload reader */
typedef struct
{
    const u8 *p;
    int len, pos, bad;
} rd_t;

static int rd_u8(rd_t *r)
{
    if (r->pos + 1 > r->len) {
        r->bad = 1;
        return 0;
    }
    return r->p[r->pos++];
}

static u16 rd_u16(rd_t *r)
{
    if (r->pos + 2 > r->len) {
        r->bad = 1;
        return 0;
    }
    u16 v = rd16(r->p + r->pos);
    r->pos += 2;
    return v;
}

static u32 rd_u32(rd_t *r)
{
    if (r->pos + 4 > r->len) {
        r->bad = 1;
        return 0;
    }
    u32 v = rd32(r->p + r->pos);
    r->pos += 4;
    return v;
}

/* reads a str into out (NUL terminated), rejects ".." components */
static void rd_str(rd_t *r, char *out, int max)
{
    u16 n = rd_u16(r);
    if (r->bad || r->pos + n > r->len || n >= max) {
        r->bad = 1;
        out[0] = 0;
        return;
    }
    memcpy(out, r->p + r->pos, n);
    out[n] = 0;
    r->pos += n;
    if (strstr(out, ".."))
        r->bad = 1;
}

static int reply(int fd, u8 cmd, s16 status, u32 id, const void *payload, u32 len)
{
    u8 h[16] = {'P', 'S', '2', 'R', 1, cmd, status & 0xff, (status >> 8) & 0xff};
    h[8] = id & 0xff;
    h[9] = (id >> 8) & 0xff;
    h[10] = (id >> 16) & 0xff;
    h[11] = id >> 24;
    h[12] = len & 0xff;
    h[13] = (len >> 8) & 0xff;
    h[14] = (len >> 16) & 0xff;
    h[15] = len >> 24;
    if (send_all(fd, h, 16) < 0)
        return -1;
    return len ? send_all(fd, payload, len) : 0;
}

static int reply_err(int fd, u8 cmd, s16 st, u32 id, const char *msg)
{
    buf_t b;
    buf_init(&b);
    buf_str(&b, msg);
    int r = reply(fd, cmd, st, id, b.data, b.len);
    buf_free(&b);
    return r;
}

static int st_from_mc(int r)
{
    return mcio_status(r);
}

/* "<what>: <reason>" for a failed card operation */
static void mc_err(char *err, int errlen, const char *what, int r)
{
    snprintf(err, errlen, "%s: %s (%d)", what, mcio_strerror(r), r);
}

/* Checks that a formatted PS2 card is present. Returns ST_OK or an error
 * status with a message in err. */
static int card_ready(int port, char *err, int errlen)
{
    mc_info_t mi;
    int r = mcio_info(port, &mi);
    if (r < 0) {
        mc_err(err, errlen, "memory card", r);
        return ST_IO;
    }
    if (!mi.present) {
        snprintf(err, errlen, "no memory card in slot %d", port + 1);
        return ST_NO_CARD;
    }
    if (mi.type != sceMcTypePS2) {
        snprintf(err, errlen, "the card in slot %d is not a PS2 memory card", port + 1);
        return ST_NO_CARD;
    }
    if (!mi.formatted) {
        snprintf(err, errlen, "the memory card in slot %d is not formatted", port + 1);
        return ST_NO_CARD;
    }
    return ST_OK;
}

/* Handle one request. Returns <0 to drop the connection. */
static int handle(int fd, u8 cmd, u32 id, const u8 *payload, int len)
{
    rd_t r = {payload, len, 0, 0};
    buf_t out;
    buf_init(&out);
    int st = ST_OK;
    char err[160] = "";
    char path[160];
    int port = 0;

    g_net_requests++;

    if (cmd == CMD_HELLO) {
        u32 caps = 1 | (g_have_mmce ? 2 : 0) | (storage_available(TARGET_USB) ? 4 : 0) | (g_have_smb ? 8 : 0);
        buf_u16(&out, 1);
        buf_str(&out, "PS2SM on PlayStation 2");
        buf_str(&out, PS2SM_VERSION);
        buf_u32(&out, caps);
        goto done;
    }
    if (cmd == CMD_BYE) {
        reply(fd, cmd, 0, id, NULL, 0);
        return -1;
    }
    port = rd_u8(&r);
    if (r.bad || port > 1) {
        st = ST_BAD_REQUEST;
        strcpy(err, "bad port");
        goto done;
    }

    switch (cmd) {
        case CMD_CARD_INFO: {
            mc_info_t mi;
            int res = mcio_info(port, &mi);
            if (res < 0) { st = ST_IO; mc_err(err, sizeof(err), "memory card", res); break; }
            buf_u8(&out, mi.present);
            buf_u8(&out, mi.type);
            buf_u8(&out, mi.formatted);
            buf_u8(&out, 0);
            buf_u32(&out, mi.free_kb);
            buf_u32(&out, mi.type == sceMcTypePS2 ? 8000 : 0);
            break;
        }
        case CMD_LIST_DIR: {
            rd_str(&r, path, sizeof(path));
            if (r.bad) { st = ST_BAD_REQUEST; strcpy(err, "bad path"); break; }
            if ((st = card_ready(port, err, sizeof(err))) != ST_OK) break;
            sceMcTblGetDir *e;
            int n;
            int res = mcio_list(port, path, &e, &n);
            if (res < 0) { st = st_from_mc(res); mc_err(err, sizeof(err), "list failed", res); break; }
            buf_u32(&out, n);
            for (int i = 0; i < n; i++) {
                buf_str(&out, (const char *)e[i].EntryName);
                buf_u16(&out, e[i].AttrFile);
                buf_u32(&out, e[i].FileSizeByte);
                buf_put(&out, &e[i]._Create, 8);
                buf_put(&out, &e[i]._Modify, 8);
            }
            free(e);
            break;
        }
        case CMD_LIST_SAVES: {
            if ((st = card_ready(port, err, sizeof(err))) != ST_OK) break;
            save_info_t *s;
            int n;
            int res = mcio_list_saves(port, &s, &n);
            if (res < 0) { st = st_from_mc(res); mc_err(err, sizeof(err), "list failed", res); break; }
            buf_u32(&out, n);
            for (int i = 0; i < n; i++) {
                buf_str(&out, s[i].name);
                buf_bstr(&out, s[i].title_sjis, s[i].title_sjis_len);
                buf_u32(&out, s[i].size);
                buf_put(&out, s[i].modified, 8);
            }
            free(s);
            snprintf(g_net_last, sizeof(g_net_last), "listed %d saves (slot %d)", n, port + 1);
            break;
        }
        case CMD_READ_FILE: {
            rd_str(&r, path, sizeof(path));
            if (r.bad) { st = ST_BAD_REQUEST; strcpy(err, "bad path"); break; }
            if ((st = card_ready(port, err, sizeof(err))) != ST_OK) break;
            u8 *d;
            int n;
            int res = mcio_read_file(port, path, &d, &n);
            if (res < 0) { st = st_from_mc(res); mc_err(err, sizeof(err), "read failed", res); break; }
            buf_put(&out, d, n);
            free(d);
            break;
        }
        case CMD_WRITE_FILE: {
            rd_str(&r, path, sizeof(path));
            u32 n = rd_u32(&r);
            if (r.bad || r.pos + (int)n != len) { st = ST_BAD_REQUEST; strcpy(err, "bad request"); break; }
            if ((st = card_ready(port, err, sizeof(err))) != ST_OK) break;
            int res = mcio_write_file(port, path, payload + r.pos, n);
            if (res < 0) { st = st_from_mc(res); mc_err(err, sizeof(err), "write failed", res); }
            break;
        }
        case CMD_MKDIR:
        case CMD_DELETE:
        case CMD_DELETE_SAVE: {
            rd_str(&r, path, sizeof(path));
            if (r.bad || !path[0] || !strcmp(path, "/")) { st = ST_BAD_REQUEST; strcpy(err, "bad path"); break; }
            if ((st = card_ready(port, err, sizeof(err))) != ST_OK) break;
            int res = cmd == CMD_MKDIR ? mcio_mkdir(port, path)
                    : cmd == CMD_DELETE ? mcio_delete(port, path)
                    : mcio_delete_tree(port, path);
            if (res < 0) { st = st_from_mc(res); mc_err(err, sizeof(err), path, res); }
            if (cmd == CMD_DELETE_SAVE && res >= 0)
                snprintf(g_net_last, sizeof(g_net_last), "deleted %s", path);
            break;
        }
        case CMD_GET_SAVE: {
            rd_str(&r, path, sizeof(path));
            if (r.bad || strchr(path, '/')) { st = ST_BAD_REQUEST; strcpy(err, "bad name"); break; }
            if ((st = card_ready(port, err, sizeof(err))) != ST_OK) break;
            int res = mcio_save_to_psu(port, path, &out);
            if (res < 0) { st = st_from_mc(res); mc_err(err, sizeof(err), path, res); }
            else snprintf(g_net_last, sizeof(g_net_last), "sent %s", path);
            break;
        }
        case CMD_PUT_SAVE: {
            int overwrite = rd_u8(&r);
            if (r.bad) { st = ST_BAD_REQUEST; strcpy(err, "bad request"); break; }
            if ((st = card_ready(port, err, sizeof(err))) != ST_OK) break;
            char name[40] = "";
            int res = mcio_psu_to_card(port, payload + r.pos, len - r.pos, overwrite, name);
            if (res < 0) {
                st = st_from_mc(res);
                if (st == ST_EXISTS)
                    snprintf(err, sizeof(err), "%s already exists", name);
                else if (st == ST_NO_SPACE)
                    snprintf(err, sizeof(err), "not enough space for %s", name);
                else
                    mc_err(err, sizeof(err), name[0] ? name : "save", res);
            } else
                snprintf(g_net_last, sizeof(g_net_last), "received %s", name);
            break;
        }
        case CMD_FORMAT: {
            int res = mcio_format(port);
            if (res < 0) { st = st_from_mc(res); mc_err(err, sizeof(err), "format failed", res); }
            break;
        }
        case CMD_MMCE_STATUS: {
            int card, ch;
            char gid[64];
            if (mmce_get(port, &card, &ch, gid, sizeof(gid)) != 0) { st = ST_UNSUPPORTED; strcpy(err, "no MMCE device in this slot"); break; }
            buf_u32(&out, card);
            buf_u32(&out, ch);
            buf_str(&out, gid);
            break;
        }
        case CMD_MMCE_SET: {
            int what = rd_u8(&r), mode = rd_u8(&r);
            rd_u8(&r);
            int num = rd_u16(&r);
            if (r.bad) { st = ST_BAD_REQUEST; strcpy(err, "bad request"); break; }
            int res = mmce_set(port, what, mode, num);
            if (res == ST_UNSUPPORTED) { st = res; strcpy(err, "no MMCE device in this slot"); break; }
            buf_u32(&out, res);
            break;
        }
        default:
            st = ST_UNSUPPORTED;
            strcpy(err, "unknown command");
    }

done:;
    int rr;
    if (st != ST_OK) {
        rr = reply_err(fd, cmd, st, id, err);
        snprintf(g_net_last, sizeof(g_net_last), "error: %s", err);
    } else
        rr = reply(fd, cmd, 0, id, out.data, out.len);
    buf_free(&out);
    return rr;
}

int net_server_start(int port)
{
    if (!g_have_net)
        return -1;
    if (listen_fd >= 0)
        return 0;
    listen_fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (listen_fd < 0)
        return -1;
    /* allow restarting the server right after it was stopped */
    int one = 1;
    setsockopt(listen_fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
    struct sockaddr_in a;
    memset(&a, 0, sizeof(a));
    a.sin_family = AF_INET;
    a.sin_port = htons(port);
    a.sin_addr.s_addr = htonl(INADDR_ANY);
    if (bind(listen_fd, (struct sockaddr *)&a, sizeof(a)) < 0 || listen(listen_fd, 2) < 0) {
        close(listen_fd);
        listen_fd = -1;
        return -1;
    }
    snprintf(g_net_status, sizeof(g_net_status), "listening on %s:%d", g_ip_str, port);
    return 0;
}

void net_server_stop(void)
{
    if (client_fd >= 0)
        close(client_fd);
    if (listen_fd >= 0)
        close(listen_fd);
    client_fd = listen_fd = -1;
    strcpy(g_net_status, "stopped");
}

void net_server_poll(int timeout_ms)
{
    if (listen_fd < 0)
        return;
    fd_set rs;
    FD_ZERO(&rs);
    FD_SET(listen_fd, &rs);
    int maxfd = listen_fd;
    if (client_fd >= 0) {
        FD_SET(client_fd, &rs);
        if (client_fd > maxfd)
            maxfd = client_fd;
    }
    struct timeval tv = {0, timeout_ms * 1000};
    if (select(maxfd + 1, &rs, NULL, NULL, &tv) <= 0)
        return;

    if (FD_ISSET(listen_fd, &rs)) {
        struct sockaddr_in peer;
        int pl = sizeof(peer);
        int c = accept(listen_fd, (struct sockaddr *)&peer, &pl);
        if (c >= 0) {
            if (client_fd >= 0)
                close(client_fd); /* newest client wins */
            client_fd = c;
            char ip[20];
            ip_to_str(peer.sin_addr, ip);
            snprintf(g_net_last, sizeof(g_net_last), "client %s connected", ip);
        }
        return;
    }
    if (client_fd >= 0 && FD_ISSET(client_fd, &rs)) {
        u8 h[16];
        if (recv_all(client_fd, h, 16) < 0 || memcmp(h, "PS2S", 4) || h[4] != 1) {
            close(client_fd);
            client_fd = -1;
            strcpy(g_net_last, "client disconnected");
            return;
        }
        u32 len = rd32(h + 12);
        u32 id = rd32(h + 8);
        if (len > MAX_PAYLOAD) {
            close(client_fd);
            client_fd = -1;
            strcpy(g_net_last, "request too large; dropped client");
            return;
        }
        u8 *payload = malloc(len ? len : 1);
        if (!payload || (len && recv_all(client_fd, payload, len) < 0)) {
            free(payload);
            close(client_fd);
            client_fd = -1;
            return;
        }
        if (handle(client_fd, h[5], id, payload, len) < 0) {
            close(client_fd);
            client_fd = -1;
            strcpy(g_net_last, "client disconnected");
        }
        free(payload);
    }
}
