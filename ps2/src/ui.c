/*
 * Minimal text UI on the debug screen, plus pad input.
 */
#include <stdio.h>
#include <stdarg.h>
#include <string.h>
#include <kernel.h>
#include <debug.h>
#include <libpad.h>
#include <delaythread.h>

#include "common.h"
#include "ui.h"

#define COL_BG 0x00201810
#define COL_TEXT 0x00E0E0E0
#define COL_DIM 0x00909090
#define COL_HI 0x0040C0FF
#define COL_TITLE 0x0080FFFF
#define COL_ERR 0x004040FF

static char pad_buf[256] __attribute__((aligned(64)));
static u32 old_btns;
static int repeat_count;

void ui_init(void)
{
    init_scr();
    scr_setbgcolor(COL_BG);
    scr_clear();
    scr_setCursor(0);
}

int ui_pad_init(void)
{
    padInit(0);
    if (padPortOpen(0, 0, pad_buf) == 0)
        return -1;
    for (int i = 0; i < 300; i++) {
        int st = padGetState(0, 0);
        if (st == PAD_STATE_STABLE || st == PAD_STATE_FINDCTP1)
            return 0;
        DelayThread(10000);
    }
    return 0;
}

static void wait_vblank(void)
{
    DelayThread(16000); /* ~one NTSC frame */
}

u32 ui_buttons(void)
{
    struct padButtonStatus b;
    wait_vblank();
    int st = padGetState(0, 0);
    if (st != PAD_STATE_STABLE && st != PAD_STATE_FINDCTP1)
        return 0;
    if (padRead(0, 0, &b) == 0)
        return 0;
    u32 now = 0xffff ^ b.btns;
    u32 pressed = now & ~old_btns;
    const u32 dirs = PAD_UP | PAD_DOWN | PAD_LEFT | PAD_RIGHT;
    if (now & dirs && (now & dirs) == (old_btns & dirs)) {
        if (++repeat_count > 18 && (repeat_count % 4) == 0)
            pressed |= now & dirs;
    } else
        repeat_count = 0;
    old_btns = now;
    return pressed;
}

static void at(int row)
{
    scr_setXY(0, row);
    scr_clearline(row);
}

void ui_header(const char *title)
{
    scr_clear();
    scr_setfontcolor(COL_TITLE);
    at(0);
    scr_printf("  PS2 Save Manager %s  |  %s\n", PS2SM_VERSION, title);
    scr_setfontcolor(COL_DIM);
    at(1);
    scr_printf("  IP %s   USB %s   SMB %s   MMCE %s\n", g_ip_str, g_have_usb ? "yes" : "no", g_have_smb ? "yes" : "no",
               g_have_mmce ? "yes" : "no");
    scr_setfontcolor(COL_TEXT);
}

void ui_footer(const char *hint)
{
    scr_setfontcolor(COL_DIM);
    at(UI_ROWS + 4);
    scr_printf("  %s", hint);
    scr_setfontcolor(COL_TEXT);
}

void ui_status(const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    at(UI_ROWS + 3);
    scr_setfontcolor(COL_HI);
    scr_printf("  ");
    scr_vprintf(fmt, ap);
    scr_setfontcolor(COL_TEXT);
    va_end(ap);
}

void ui_line(int row, int highlight, const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    at(row + 3);
    scr_setfontcolor(highlight ? COL_HI : COL_TEXT);
    scr_printf(highlight ? " > " : "   ");
    scr_vprintf(fmt, ap);
    scr_setfontcolor(COL_TEXT);
    va_end(ap);
}

void ui_message(const char *title, const char *fmt, ...)
{
    char text[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(text, sizeof(text), fmt, ap);
    va_end(ap);
    ui_header(title);
    int row = 1;
    char *line = strtok(text, "\n");
    while (line && row < UI_ROWS) {
        ui_line(row++, 0, "%s", line);
        line = strtok(NULL, "\n");
    }
    ui_footer("X / O: continue");
    while (!(ui_buttons() & (BTN_CROSS | BTN_CIRCLE)))
        ;
}

int ui_confirm(const char *fmt, ...)
{
    char text[256];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(text, sizeof(text), fmt, ap);
    va_end(ap);
    ui_header("Confirm");
    ui_line(2, 0, "%s", text);
    ui_footer("X: yes    O: no");
    for (;;) {
        u32 b = ui_buttons();
        if (b & BTN_CROSS)
            return 1;
        if (b & BTN_CIRCLE)
            return 0;
    }
}

int ui_list(const char *title, const char *hint, int count, ui_item_fn item, void *ctx, int *cursor, u32 accept,
            u32 *pressed)
{
    int cur = cursor ? *cursor : 0;
    int top = 0;
    char line[128];
    int redraw = 1;
    if (cur >= count)
        cur = count ? count - 1 : 0;
    for (;;) {
        if (redraw) {
            ui_header(title);
            if (count == 0)
                ui_line(1, 0, "(empty)");
            if (cur < top)
                top = cur;
            if (cur >= top + UI_ROWS)
                top = cur - UI_ROWS + 1;
            for (int i = 0; i < UI_ROWS && top + i < count; i++) {
                item(top + i, line, sizeof(line), ctx);
                ui_line(i, top + i == cur, "%s", line);
            }
            if (count > UI_ROWS)
                ui_status("%d / %d", cur + 1, count);
            ui_footer(hint);
            redraw = 0;
        }
        u32 b = ui_buttons();
        if (!b)
            continue;
        if (b & BTN_UP && count) {
            cur = (cur + count - 1) % count;
            redraw = 1;
        } else if (b & BTN_DOWN && count) {
            cur = (cur + 1) % count;
            redraw = 1;
        } else if (b & BTN_LEFT && count) {
            cur = cur > UI_ROWS ? cur - UI_ROWS : 0;
            redraw = 1;
        } else if (b & BTN_RIGHT && count) {
            cur = cur + UI_ROWS < count ? cur + UI_ROWS : count - 1;
            redraw = 1;
        } else if (b & BTN_CIRCLE) {
            if (cursor)
                *cursor = cur;
            return -1;
        } else if (b & accept && (count || !(b & BTN_CROSS))) {
            if (cursor)
                *cursor = cur;
            if (pressed)
                *pressed = b & accept;
            return count ? cur : -1;
        }
    }
}
