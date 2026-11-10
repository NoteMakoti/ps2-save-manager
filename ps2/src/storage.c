/*
 * Backup targets (USB mass storage, SMB share) and MMCE control.
 */
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <fileXio_rpc.h>
#include <iox_stat.h>
#include <ps2smb.h>

#include "storage.h"

/* mmceman devctl commands (ps2-mmce/mmceman mmce_cmds.h) */
#define MMCE_CMD_PING 0x1
#define MMCE_CMD_GET_CARD 0x3
#define MMCE_CMD_SET_CARD 0x4
#define MMCE_CMD_GET_CHANNEL 0x5
#define MMCE_CMD_SET_CHANNEL 0x6
#define MMCE_CMD_GET_GAMEID 0x7

const char *storage_name(target_t t)
{
    return t == TARGET_USB ? "USB" : "SMB";
}

void storage_root(target_t t, char *out, int n)
{
    if (t == TARGET_USB)
        snprintf(out, n, "mass:/%s", PS2SM_DIR);
    else
        snprintf(out, n, "smb:/%s", g_cfg.smb_dir[0] ? g_cfg.smb_dir : PS2SM_DIR);
}

int storage_available(target_t t)
{
    if (t == TARGET_SMB)
        return g_have_smb;
    if (!g_have_usb)
        return 0;
    iox_stat_t st;
    return fileXioGetStat("mass:/", &st) >= 0;
}

int smb_connect(char *err, int errlen)
{
    if (!g_have_net) {
        snprintf(err, errlen, "network is not up");
        return -1;
    }
    if (!g_cfg.smb_ip[0] || !g_cfg.smb_share[0]) {
        snprintf(err, errlen, "set smb_ip and smb_share in PS2SM.CNF");
        return -1;
    }
    static smbLogOn_in_t logon __attribute__((aligned(64)));
    memset(&logon, 0, sizeof(logon));
    strncpy(logon.serverIP, g_cfg.smb_ip, sizeof(logon.serverIP) - 1);
    logon.serverPort = g_cfg.smb_port;
    strncpy(logon.User, g_cfg.smb_user, sizeof(logon.User) - 1);
    strncpy(logon.Password, g_cfg.smb_pass, sizeof(logon.Password) - 1);
    logon.PasswordType = g_cfg.smb_pass[0] ? PLAINTEXT_PASSWORD : NO_PASSWORD;
    int r = fileXioDevctl("smb:", SMB_DEVCTL_LOGON, &logon, sizeof(logon), NULL, 0);
    if (r < 0) {
        snprintf(err, errlen, "SMB logon to %s failed (%d)%s", g_cfg.smb_ip, r,
                 r == -SMB_DEVCTL_LOGON_ERR_PROT ? ": server needs SMB1/NT LM 0.12" : "");
        return -1;
    }
    static smbOpenShare_in_t os __attribute__((aligned(64)));
    memset(&os, 0, sizeof(os));
    strncpy(os.ShareName, g_cfg.smb_share, sizeof(os.ShareName) - 1);
    strncpy(os.Password, g_cfg.smb_pass, sizeof(os.Password) - 1);
    os.PasswordType = g_cfg.smb_pass[0] ? PLAINTEXT_PASSWORD : NO_PASSWORD;
    r = fileXioDevctl("smb:", SMB_DEVCTL_OPENSHARE, &os, sizeof(os), NULL, 0);
    if (r < 0) {
        snprintf(err, errlen, "cannot open share '%s' (%d)", g_cfg.smb_share, r);
        fileXioDevctl("smb:", SMB_DEVCTL_LOGOFF, NULL, 0, NULL, 0);
        return -1;
    }
    char root[160];
    storage_root(TARGET_SMB, root, sizeof(root));
    fileXioMkdir(root, 0777);
    g_have_smb = 1;
    return 0;
}

static int has_psu_ext(const char *n)
{
    size_t l = strlen(n);
    return l > 4 && !strcasecmp(n + l - 4, ".psu");
}

static int cmp_str(const void *a, const void *b)
{
    return strcasecmp(*(char *const *)a, *(char *const *)b);
}

int storage_list_psu(target_t t, char ***names, int *count)
{
    char root[160];
    storage_root(t, root, sizeof(root));
    int fd = fileXioDopen(root);
    *names = NULL;
    *count = 0;
    if (fd < 0)
        return fd;
    static iox_dirent_t de __attribute__((aligned(64)));
    int cap = 0;
    while (fileXioDread(fd, &de) > 0) {
        if (FIO_S_ISDIR(de.stat.mode) || !has_psu_ext(de.name))
            continue;
        if (*count == cap) {
            cap = cap ? cap * 2 : 32;
            *names = realloc(*names, cap * sizeof(char *));
        }
        (*names)[(*count)++] = strdup(de.name);
    }
    fileXioDclose(fd);
    if (*count)
        qsort(*names, *count, sizeof(char *), cmp_str);
    return 0;
}

void storage_free_list(char **names, int count)
{
    for (int i = 0; i < count; i++)
        free(names[i]);
    free(names);
}

int storage_write(target_t t, const char *name, const u8 *data, int len)
{
    char root[160], p[320];
    storage_root(t, root, sizeof(root));
    fileXioMkdir(root, 0777);
    snprintf(p, sizeof(p), "%s/%s", root, name);
    int fd = fileXioOpen(p, IOX_WRONLY | IOX_CREAT | IOX_TRUNC, 0666);
    if (fd < 0)
        return fd;
    int put = 0;
    while (put < len) {
        int n = len - put > 32768 ? 32768 : len - put;
        int r = fileXioWrite(fd, data + put, n);
        if (r <= 0)
            break;
        put += r;
    }
    fileXioClose(fd);
    return put == len ? 0 : -1;
}

int storage_read(target_t t, const char *name, u8 **data, int *len)
{
    char root[160], p[320];
    storage_root(t, root, sizeof(root));
    snprintf(p, sizeof(p), "%s/%s", root, name);
    int fd = fileXioOpen(p, IOX_RDONLY);
    if (fd < 0)
        return fd;
    int size = fileXioLseek(fd, 0, SEEK_END);
    fileXioLseek(fd, 0, SEEK_SET);
    if (size <= 0 || size > 32 * 1024 * 1024) {
        fileXioClose(fd);
        return -1;
    }
    u8 *b = malloc(size);
    int got = 0;
    while (got < size) {
        int r = fileXioRead(fd, b + got, size - got > 32768 ? 32768 : size - got);
        if (r <= 0)
            break;
        got += r;
    }
    fileXioClose(fd);
    if (got != size) {
        free(b);
        return -1;
    }
    *data = b;
    *len = size;
    return 0;
}

/* ------------------------------------------------------------------ */

static const char *mmce_dev(int port)
{
    return port ? "mmce1:" : "mmce0:";
}

int mmce_present(int port)
{
    if (!g_have_mmce)
        return 0;
    return fileXioDevctl(mmce_dev(port), MMCE_CMD_PING, NULL, 0, NULL, 0) >= 0;
}

int mmce_get(int port, int *card, int *channel, char *gameid, int gidlen)
{
    if (!mmce_present(port))
        return ST_UNSUPPORTED;
    *card = fileXioDevctl(mmce_dev(port), MMCE_CMD_GET_CARD, NULL, 0, NULL, 0);
    *channel = fileXioDevctl(mmce_dev(port), MMCE_CMD_GET_CHANNEL, NULL, 0, NULL, 0);
    static char buf[256] __attribute__((aligned(64)));
    memset(buf, 0, sizeof(buf));
    if (fileXioDevctl(mmce_dev(port), MMCE_CMD_GET_GAMEID, NULL, 0, buf, sizeof(buf)) >= 0) {
        strncpy(gameid, buf, gidlen - 1);
        gameid[gidlen - 1] = 0;
    } else
        gameid[0] = 0;
    return 0;
}

int mmce_set(int port, int what, int mode, int num)
{
    if (!mmce_present(port))
        return ST_UNSUPPORTED;
    static u32 arg __attribute__((aligned(64)));
    if (what == 0) {
        arg = ((u32)0 << 24) | ((u32)(mode & 0xff) << 16) | (num & 0xffff);
        return fileXioDevctl(mmce_dev(port), MMCE_CMD_SET_CARD, &arg, 4, NULL, 0);
    }
    arg = ((u32)(mode & 0xff) << 16) | (num & 0xffff);
    return fileXioDevctl(mmce_dev(port), MMCE_CMD_SET_CHANNEL, &arg, 4, NULL, 0);
}
