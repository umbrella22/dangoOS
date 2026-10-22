/*
 * Lua's os library (vendor/lua-5.5.1/loslib.c, unchanged) with the process
 * and file calls routed away from libc: there is no shell, no file system yet
 * and no process to exit. time() and clock() read the host clock.
 *
 *   os.execute()      -> false (no shell), os.execute(cmd) -> fails
 *   os.remove/rename  -> fail with "not supported" until storage lands (L2)
 *   os.tmpname        -> error
 *   os.exit           -> removed by the glue (it would end the firmware)
 */
#include <errno.h>
#include <stdint.h>
#include <locale.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

extern int64_t mosaico_host_now_us(void);
extern int64_t mosaico_host_unix_us(void);

static time_t mosaico_os_time(time_t *out)
{
    time_t t = (time_t)(mosaico_host_unix_us() / 1000000);
    if (out) *out = t;
    return t;
}

static clock_t mosaico_os_clock(void)
{
    return (clock_t)(mosaico_host_now_us() / (1000000 / CLOCKS_PER_SEC));
}

static int mosaico_os_unsupported(void)
{
    errno = ENOSYS;
    return -1;
}

#define l_system(cmd) ((cmd) == NULL ? 0 : -1)
#define lua_tmpnam(b, e) { (b)[0] = '\0'; e = 1; }
#define LUA_TMPNAMBUFSIZE 32
#define time(p) mosaico_os_time(p)
#define clock() mosaico_os_clock()
#define remove(f) ((void)(f), mosaico_os_unsupported())
#define rename(a, b) ((void)(a), (void)(b), mosaico_os_unsupported())
#define exit(s) abort()

#include "loslib.c"
