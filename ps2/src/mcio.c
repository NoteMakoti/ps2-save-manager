/*
 * Memory card access through libmc (mcman/mcserv on the IOP).
 */
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <libmc.h>

#include "mcio.h"

#define CHUNK 8192

static int sync_ret(void)
{
    int ret;
    mcSync(0, NULL, &ret);
    return ret;
}

int mcio_init(void)
{
    return mcInit(MC_TYPE_XMC);
}

int mcio_status(int r)
{
    switch (r) {
        case sceMcResSucceed: return ST_OK;
        case sceMcResNoEntry: return ST_NOT_FOUND;
        case sceMcResFullDevice: return ST_NO_SPACE;
        case sceMcResNoFormat: return ST_NO_CARD;
        case sceMcResNotEmpty: return ST_BAD_REQUEST;
        case sceMcResDeniedPermit: return ST_FAIL;
        default: return r < 0 ? ST_IO : ST_OK;
    }
}

int mcio_info(int port, mc_info_t *info)
{
    int type = 0, freec = 0, format = 0, ret;
    memset(info, 0, sizeof(*info));
    /* The first call after a card change reports -1/-2; ask twice. */
    for (int tries = 0; tries < 2; tries++) {
        mcGetInfo(port, 0, &type, &freec, &format);
        ret = sync_ret();
        if (ret == 0 || ret == -1 || ret == -2)
            break;
    }
    if (ret < -2 || type == sceMcTypeNoCard)
        return 0;
    info->present = 1;
    info->type = type;
    info->formatted = (ret == -2) ? 0 : (format != 0);
    info->free_kb = freec;
    return 0;
}

int mcio_list(int port, const char *path, sceMcTblGetDir **out, int *count)
{
    static sceMcTblGetDir table[MCIO_MAX_ENTRIES] __attribute__((aligned(64)));
    char pattern[160];
    if (!strcmp(path, "/") || !path[0])
        strcpy(pattern, "/*");
    else
        snprintf(pattern, sizeof(pattern), "%s%s/*", path[0] == '/' ? "" : "/", path);

    mcGetDir(port, 0, pattern, 0, MCIO_MAX_ENTRIES, table);
    int n = sync_ret();
    if (n < 0)
        return n;
    sceMcTblGetDir *res = malloc(sizeof(sceMcTblGetDir) * (n ? n : 1));
    int k = 0;
    for (int i = 0; i < n; i++) {
        const char *nm = (const char *)table[i].EntryName;
        if (!strcmp(nm, ".") || !strcmp(nm, ".."))
            continue;
        res[k++] = table[i];
    }
    *out = res;
    *count = k;
    return 0;
}

static void abs_path(const char *in, char *out, int n)
{
    snprintf(out, n, "%s%s", in[0] == '/' ? "" : "/", in);
}

int mcio_read_file(int port, const char *path, u8 **data, int *len)
{
    char p[160];
    abs_path(path, p, sizeof(p));
    mcOpen(port, 0, p, IOX_RDONLY);
    int fd = sync_ret();
    if (fd < 0)
        return fd;
    mcSeek(fd, 0, SEEK_END);
    int size = sync_ret();
    mcSeek(fd, 0, SEEK_SET);
    sync_ret();
    if (size < 0) {
        mcClose(fd);
        sync_ret();
        return size;
    }
    u8 *b = malloc(size ? size : 1);
    int got = 0, r = 0;
    while (got < size) {
        int want = size - got > CHUNK ? CHUNK : size - got;
        mcRead(fd, b + got, want);
        r = sync_ret();
        if (r <= 0)
            break;
        got += r;
    }
    mcClose(fd);
    sync_ret();
    if (got != size) {
        free(b);
        return r < 0 ? r : sceMcResFailDetect;
    }
    *data = b;
    *len = size;
    return 0;
}

int mcio_write_file(int port, const char *path, const u8 *data, int len)
{
    char p[160];
    abs_path(path, p, sizeof(p));
    /* replace: delete first so the old clusters are released */
    mcDelete(port, 0, p);
    sync_ret();
    mcOpen(port, 0, p, IOX_WRONLY | IOX_CREAT);
    int fd = sync_ret();
    if (fd < 0)
        return fd;
    int put = 0, r = 0;
    while (put < len) {
        int n = len - put > CHUNK ? CHUNK : len - put;
        mcWrite(fd, data + put, n);
        r = sync_ret();
        if (r <= 0)
            break;
        put += r;
    }
    mcClose(fd);
    sync_ret();
    if (put != len)
        return r < 0 ? r : sceMcResFullDevice;
    return 0;
}

int mcio_mkdir(int port, const char *path)
{
    char p[160];
    abs_path(path, p, sizeof(p));
    mcMkDir(port, 0, p);
    return sync_ret();
}

int mcio_delete(int port, const char *path)
{
    char p[160];
    abs_path(path, p, sizeof(p));
    mcDelete(port, 0, p);
    return sync_ret();
}

int mcio_delete_tree(int port, const char *dir)
{
    sceMcTblGetDir *ents;
    int n;
    int r = mcio_list(port, dir, &ents, &n);
    if (r < 0)
        return r;
    char p[160];
    for (int i = 0; i < n; i++) {
        snprintf(p, sizeof(p), "%s/%s", dir, ents[i].EntryName);
        if (ents[i].AttrFile & DF_DIR)
            r = mcio_delete_tree(port, p);
        else
            r = mcio_delete(port, p);
        if (r < 0) {
            free(ents);
            return r;
        }
    }
    free(ents);
    return mcio_delete(port, dir);
}

int mcio_format(int port)
{
    mcFormat(port, 0);
    return sync_ret();
}

int mcio_list_saves(int port, save_info_t **out, int *count)
{
    sceMcTblGetDir *root;
    int n;
    int r = mcio_list(port, "/", &root, &n);
    if (r < 0)
        return r;
    save_info_t *s = calloc(n ? n : 1, sizeof(save_info_t));
    int k = 0;
    for (int i = 0; i < n; i++) {
        if (!(root[i].AttrFile & DF_DIR))
            continue;
        save_info_t *si = &s[k++];
        strncpy(si->name, (const char *)root[i].EntryName, 32);
        si->mode = root[i].AttrFile;
        memcpy(si->created, &root[i]._Create, 8);
        memcpy(si->modified, &root[i]._Modify, 8);

        sceMcTblGetDir *files;
        int nf;
        if (mcio_list(port, si->name, &files, &nf) == 0) {
            for (int j = 0; j < nf; j++)
                si->size += files[j].FileSizeByte;
            free(files);
        }
        char ip[80];
        snprintf(ip, sizeof(ip), "%s/icon.sys", si->name);
        u8 *icon;
        int il;
        if (mcio_read_file(port, ip, &icon, &il) == 0) {
            if (il >= 964 && !memcmp(icon, "PS2D", 4)) {
                const u8 *t = icon + 0xC0;
                int tl = 0;
                while (tl < 68 && t[tl])
                    tl++;
                int lb = icon[6] | (icon[7] << 8);
                if (lb > tl)
                    lb = tl;
                memcpy(si->title_sjis, t, lb);
                si->title_sjis[lb] = ' ';
                memcpy(si->title_sjis + lb + 1, t + lb, tl - lb);
                si->title_sjis_len = tl + 1;
                sjis_to_ascii(si->title_sjis, si->title_sjis_len, si->title, sizeof(si->title));
            }
            free(icon);
        }
    }
    free(root);
    *out = s;
    *count = k;
    return 0;
}

/* ------------------------------------------------------------------ */
/* .psu images: 512-byte entries; data padded to 1 KiB.                */

static void put_dirent(buf_t *b, u16 mode, u32 length, const void *created, const void *modified, const char *name)
{
    u8 e[512];
    memset(e, 0, sizeof(e));
    e[0] = mode & 0xff;
    e[1] = mode >> 8;
    e[4] = length & 0xff;
    e[5] = (length >> 8) & 0xff;
    e[6] = (length >> 16) & 0xff;
    e[7] = length >> 24;
    memcpy(e + 8, created, 8);
    memcpy(e + 24, modified, 8);
    strncpy((char *)e + 0x40, name, 32);
    buf_put(b, e, 512);
}

static u32 rd32(const u8 *p) { return p[0] | (p[1] << 8) | (p[2] << 16) | ((u32)p[3] << 24); }

int mcio_save_to_psu(int port, const char *dir, buf_t *out)
{
    /* find the directory's own attributes */
    sceMcTblGetDir *root;
    int nr;
    int r = mcio_list(port, "/", &root, &nr);
    if (r < 0)
        return r;
    sceMcTblGetDir self;
    int found = 0;
    for (int i = 0; i < nr; i++)
        if (!strcmp((const char *)root[i].EntryName, dir)) {
            self = root[i];
            found = 1;
        }
    free(root);
    if (!found || !(self.AttrFile & DF_DIR))
        return sceMcResNoEntry;

    sceMcTblGetDir *files;
    int nf;
    r = mcio_list(port, dir, &files, &nf);
    if (r < 0)
        return r;
    u16 dmode = self.AttrFile | DF_EXISTS | DF_DIR;
    put_dirent(out, dmode, nf + 2, &self._Create, &self._Modify, dir);
    put_dirent(out, DF_SAVE_DIR, 0, &self._Create, &self._Modify, ".");
    put_dirent(out, DF_SAVE_DIR, 0, &self._Create, &self._Modify, "..");
    char p[160];
    for (int i = 0; i < nf; i++) {
        if (files[i].AttrFile & DF_DIR)
            continue; /* saves have no subdirectories */
        snprintf(p, sizeof(p), "%s/%s", dir, files[i].EntryName);
        u8 *data;
        int len;
        r = mcio_read_file(port, p, &data, &len);
        if (r < 0) {
            free(files);
            return r;
        }
        put_dirent(out, files[i].AttrFile | DF_EXISTS | DF_FILE, len, &files[i]._Create, &files[i]._Modify,
                   (const char *)files[i].EntryName);
        buf_put(out, data, len);
        buf_zero(out, ((len + 1023) & ~1023) - len);
        free(data);
    }
    free(files);
    return 0;
}

int psu_get_dirname(const u8 *psu, int len, char *out)
{
    if (len < 1536)
        return -1;
    u16 mode = psu[0] | (psu[1] << 8);
    if ((mode & (DF_DIR | DF_EXISTS)) != (DF_DIR | DF_EXISTS))
        return -1;
    if (strncmp((const char *)psu + 512 + 0x40, ".", 2) || strncmp((const char *)psu + 1024 + 0x40, "..", 3))
        return -1;
    memcpy(out, psu + 0x40, 32);
    out[32] = 0;
    return 0;
}

static void fill_info(sceMcTblGetDir *info, const u8 *ent)
{
    memset(info, 0, sizeof(*info));
    memcpy(&info->_Create, ent + 8, 8);
    memcpy(&info->_Modify, ent + 24, 8);
    info->AttrFile = ent[0] | (ent[1] << 8);
}

int mcio_psu_to_card(int port, const u8 *psu, int len, int overwrite, char *dirname_out)
{
    char dir[33];
    if (psu_get_dirname(psu, len, dir) < 0)
        return -1000; /* bad request */
    if (dirname_out)
        strcpy(dirname_out, dir);
    u32 count = rd32(psu + 4);
    if (count < 2)
        return -1000;
    count -= 2;

    /* validate and measure */
    int off = 1536;
    int need_kb = (count + 2 + 1) / 2;
    for (u32 i = 0; i < count; i++) {
        if (off + 512 > len)
            return -1000;
        u32 fl = rd32(psu + off + 4);
        int padded = (fl + 1023) & ~1023;
        if (off + 512 + (int)fl > len)
            return -1000;
        need_kb += padded / 1024;
        off += 512 + padded;
        if (off > len)
            off = len; /* tolerate missing tail padding */
    }

    mc_info_t mi;
    mcio_info(port, &mi);
    if (!mi.present)
        return sceMcResNoFormat;

    char p[160];
    snprintf(p, sizeof(p), "/%s", dir);
    sceMcTblGetDir *tmp;
    int tn;
    int exists = mcio_list(port, dir, &tmp, &tn) == 0;
    if (exists) {
        int old_kb = (tn + 3) / 2;
        for (int i = 0; i < tn; i++)
            old_kb += (tmp[i].FileSizeByte + 1023) / 1024;
        free(tmp);
        if (!overwrite)
            return -1001; /* exists */
        if (mi.free_kb + old_kb < need_kb)
            return sceMcResFullDevice;
        int r = mcio_delete_tree(port, p);
        if (r < 0)
            return r;
    } else if (mi.free_kb < need_kb) {
        return sceMcResFullDevice;
    }

    int r = mcio_mkdir(port, p);
    if (r < 0)
        return r;

    off = 1536;
    sceMcTblGetDir info __attribute__((aligned(64)));
    for (u32 i = 0; i < count; i++) {
        const u8 *ent = psu + off;
        u32 fl = rd32(ent + 4);
        char name[33];
        memcpy(name, ent + 0x40, 32);
        name[32] = 0;
        snprintf(p, sizeof(p), "/%s/%s", dir, name);
        r = mcio_write_file(port, p, ent + 512, fl);
        if (r < 0)
            return r;
        fill_info(&info, ent);
        mcSetFileInfo(port, 0, p, &info, 0x01 | 0x02 | 0x04);
        sync_ret();
        off += 512 + ((fl + 1023) & ~1023);
    }
    /* directory timestamps last, after the files touched it */
    snprintf(p, sizeof(p), "/%s", dir);
    fill_info(&info, psu);
    mcSetFileInfo(port, 0, p, &info, 0x01 | 0x02 | 0x04);
    sync_ret();
    return 0;
}
