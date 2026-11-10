/*
 * Buffers, config file, Shift-JIS folding.
 */
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <ctype.h>
#include <fileXio_rpc.h>
#include <libmc.h>

#include "common.h"
#include "mcio.h"

config_t g_cfg;
int g_have_net, g_have_usb, g_have_smb, g_have_mmce;
char g_ip_str[32] = "-";
char g_boot_dir[256] = "";

/* ------------------------------------------------------------------ */

void buf_init(buf_t *b)
{
    b->data = NULL;
    b->len = b->cap = 0;
}

void buf_free(buf_t *b)
{
    free(b->data);
    buf_init(b);
}

int buf_reserve(buf_t *b, size_t extra)
{
    if (b->len + extra <= b->cap)
        return 0;
    size_t nc = b->cap ? b->cap : 4096;
    while (nc < b->len + extra)
        nc *= 2;
    u8 *n = realloc(b->data, nc);
    if (!n)
        return -1;
    b->data = n;
    b->cap = nc;
    return 0;
}

int buf_put(buf_t *b, const void *p, size_t n)
{
    if (buf_reserve(b, n))
        return -1;
    if (n)
        memcpy(b->data + b->len, p, n);
    b->len += n;
    return 0;
}

int buf_zero(buf_t *b, size_t n)
{
    if (buf_reserve(b, n))
        return -1;
    memset(b->data + b->len, 0, n);
    b->len += n;
    return 0;
}

int buf_u8(buf_t *b, u8 v) { return buf_put(b, &v, 1); }

int buf_u16(buf_t *b, u16 v)
{
    u8 t[2] = {v & 0xff, v >> 8};
    return buf_put(b, t, 2);
}

int buf_u32(buf_t *b, u32 v)
{
    u8 t[4] = {v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, v >> 24};
    return buf_put(b, t, 4);
}

int buf_bstr(buf_t *b, const void *p, u16 n)
{
    if (buf_u16(b, n))
        return -1;
    return buf_put(b, p, n);
}

int buf_str(buf_t *b, const char *s) { return buf_bstr(b, s, (u16)strlen(s)); }

/* ------------------------------------------------------------------ */

void config_defaults(config_t *c)
{
    memset(c, 0, sizeof(*c));
    c->use_dhcp = 1;
    strcpy(c->ip, "192.168.0.10");
    strcpy(c->netmask, "255.255.255.0");
    strcpy(c->gateway, "192.168.0.1");
    c->port = 6524;
    c->smb_port = 445;
    strcpy(c->smb_user, "GUEST");
    strcpy(c->smb_dir, "PS2SM");
}

static void trim(char *s)
{
    char *p = s;
    while (*p && isspace((unsigned char)*p))
        p++;
    memmove(s, p, strlen(p) + 1);
    size_t n = strlen(s);
    while (n && isspace((unsigned char)s[n - 1]))
        s[--n] = 0;
}

static void parse_config(config_t *c, char *text)
{
    char *line = strtok(text, "\n");
    while (line) {
        char *eq = strchr(line, '=');
        if (eq && line[0] != '#' && line[0] != ';') {
            *eq = 0;
            char *k = line, *v = eq + 1;
            trim(k);
            trim(v);
#define SET(name, field) else if (!strcasecmp(k, name)) { strncpy(c->field, v, sizeof(c->field) - 1); }
            if (!strcasecmp(k, "ip")) {
                if (!strcasecmp(v, "dhcp"))
                    c->use_dhcp = 1;
                else {
                    c->use_dhcp = 0;
                    strncpy(c->ip, v, sizeof(c->ip) - 1);
                }
            }
            SET("netmask", netmask)
            SET("gateway", gateway)
            SET("smb_ip", smb_ip)
            SET("smb_share", smb_share)
            SET("smb_user", smb_user)
            SET("smb_pass", smb_pass)
            SET("smb_dir", smb_dir)
            else if (!strcasecmp(k, "port")) c->port = atoi(v);
            else if (!strcasecmp(k, "smb_port")) c->smb_port = atoi(v);
#undef SET
        }
        line = strtok(NULL, "\n");
    }
}

static int read_text(const char *path, char *out, int max)
{
    int n = -1;
    if (!strncmp(path, "mc", 2)) {
        u8 *data;
        int len;
        int port = path[2] - '0';
        if (mcio_read_file(port, path + 4, &data, &len) == 0) {
            n = len < max - 1 ? len : max - 1;
            memcpy(out, data, n);
            free(data);
        }
    } else {
        int fd = fileXioOpen(path, IOX_RDONLY);
        if (fd >= 0) {
            n = fileXioRead(fd, out, max - 1);
            fileXioClose(fd);
        }
    }
    if (n >= 0)
        out[n] = 0;
    return n;
}

/* Looks next to the ELF, then on the memory cards, then on USB. */
int config_load(config_t *c)
{
    static char text[4096];
    char path[300];
    config_defaults(c);
    const char *tries[] = {"mc0:/PS2SM/PS2SM.CNF", "mc1:/PS2SM/PS2SM.CNF", "mass:/PS2SM/PS2SM.CNF"};
    if (g_boot_dir[0]) {
        snprintf(path, sizeof(path), "%sPS2SM.CNF", g_boot_dir);
        if (read_text(path, text, sizeof(text)) > 0) {
            parse_config(c, text);
            return 1;
        }
    }
    for (int i = 0; i < 3; i++) {
        if (read_text(tries[i], text, sizeof(text)) > 0) {
            parse_config(c, text);
            return 1;
        }
    }
    return 0;
}

/* ------------------------------------------------------------------ */

/* Full-width ASCII and common punctuation to plain ASCII. Everything
 * else becomes '?', since the debug font only has ASCII glyphs. */
void sjis_to_ascii(const u8 *in, int inlen, char *out, int outlen)
{
    int o = 0;
    for (int i = 0; i < inlen && in[i] && o < outlen - 1;) {
        u8 c = in[i];
        if (c < 0x80) {
            out[o++] = c;
            i++;
            continue;
        }
        if (c >= 0xA1 && c <= 0xDF) { /* half-width katakana */
            out[o++] = '?';
            i++;
            continue;
        }
        if (i + 1 >= inlen)
            break;
        u16 w = (c << 8) | in[i + 1];
        i += 2;
        char a = 0;
        if (w == 0x8140) a = ' ';
        else if (w >= 0x824F && w <= 0x8258) a = '0' + (w - 0x824F);
        else if (w >= 0x8260 && w <= 0x8279) a = 'A' + (w - 0x8260);
        else if (w >= 0x8281 && w <= 0x829A) a = 'a' + (w - 0x8281);
        else switch (w) {
            case 0x8143: a = ','; break;
            case 0x8144: a = '.'; break;
            case 0x8146: a = ':'; break;
            case 0x8147: a = ';'; break;
            case 0x8148: a = '?'; break;
            case 0x8149: a = '!'; break;
            case 0x815B: case 0x815C: case 0x815D: case 0x817C: a = '-'; break;
            case 0x815E: a = '/'; break;
            case 0x8165: case 0x8166: a = '\''; break;
            case 0x8167: case 0x8168: a = '"'; break;
            case 0x8169: a = '('; break;
            case 0x816A: a = ')'; break;
            case 0x816D: a = '['; break;
            case 0x816E: a = ']'; break;
            case 0x817B: a = '+'; break;
            case 0x8181: a = '='; break;
            case 0x8193: a = '%'; break;
            case 0x8194: a = '#'; break;
            case 0x8195: a = '&'; break;
            case 0x8196: a = '*'; break;
            case 0x8197: a = '@'; break;
            default: a = '?';
        }
        out[o++] = a;
    }
    out[o] = 0;
}
