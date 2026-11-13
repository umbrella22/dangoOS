/* newlib's file syscalls over the Rust VFS (src/global.rs).
 *
 * Firmware only. newlib's stdio (fopen, fread, ...) and the reentrant
 * wrappers end here; directories and mkdir have no newlib implementation
 * on ESP (ESP-IDF's VFS supplies them), so they are defined here too.
 * Descriptors 0-2 are the console: stdin reads as empty, stdout/stderr go
 * to the log through mosaico_console_write (mosaico-s31). */
#include <errno.h>
#include <fcntl.h>
#include <reent.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/dirent.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/types.h>
#include <unistd.h>

extern int mosaico_vfs_open(const char *path, int flags);
extern int mosaico_vfs_close(int fd);
extern int mosaico_vfs_read(int fd, void *buf, size_t n);
extern int mosaico_vfs_write(int fd, const void *buf, size_t n);
extern int64_t mosaico_vfs_lseek(int fd, int64_t off, int whence);
extern int mosaico_vfs_fstat(int fd, int *kind, uint32_t *size);
extern int mosaico_vfs_stat(const char *path, int *kind, uint32_t *size);
extern int mosaico_vfs_mkdir(const char *path);
extern int mosaico_vfs_rmdir(const char *path);
extern int mosaico_vfs_unlink(const char *path);
extern int mosaico_vfs_rename(const char *a, const char *b);
extern int mosaico_vfs_fsync(int fd);
extern int mosaico_vfs_ftruncate(int fd, uint32_t size);
extern int mosaico_vfs_opendir(const char *path);
extern int mosaico_vfs_readdir(int h, char *name, int *kind);
extern int mosaico_vfs_closedir(int h);
extern int mosaico_vfs_statvfs(const char *path, uint32_t *bsize, uint32_t *blocks, uint32_t *bfree);

extern void mosaico_console_write(const char *buf, size_t n);

/* Result < 0 is -errno: set errno, return -1. */
static int ret(struct _reent *r, int v)
{
    if (v < 0) {
        if (r) r->_errno = -v;
        errno = -v;
        return -1;
    }
    return v;
}

static int to_vfs_flags(int f)
{
    int acc = f & O_ACCMODE;
    int out = 0;
    if (acc == O_RDONLY || acc == O_RDWR) out |= 1;
    if (acc == O_WRONLY || acc == O_RDWR) out |= 2;
    if (f & O_CREAT) out |= 4;
    if (f & O_TRUNC) out |= 8;
    if (f & O_APPEND) out |= 16;
    if (f & O_EXCL) out |= 32;
    return out;
}

static void fill_stat(struct stat *st, int kind, uint32_t size)
{
    memset(st, 0, sizeof(*st));
    st->st_mode = (kind == 2 ? S_IFDIR : S_IFREG) | 0777;
    st->st_size = (off_t)size;
    st->st_nlink = 1;
    st->st_blksize = 4096;
    st->st_blocks = (size + 511) / 512;
}

int _open_r(struct _reent *r, const char *path, int flags, int mode)
{
    (void)mode;
    return ret(r, mosaico_vfs_open(path, to_vfs_flags(flags)));
}

int _close_r(struct _reent *r, int fd)
{
    if (fd >= 0 && fd < 3) return 0;
    return ret(r, mosaico_vfs_close(fd));
}

_ssize_t _read_r(struct _reent *r, int fd, void *buf, size_t n)
{
    if (fd == 0) return 0;
    if (fd > 0 && fd < 3) return ret(r, -EBADF);
    return ret(r, mosaico_vfs_read(fd, buf, n));
}

_ssize_t _write_r(struct _reent *r, int fd, const void *buf, size_t n)
{
    if (fd == 1 || fd == 2) {
        mosaico_console_write(buf, n);
        return (_ssize_t)n;
    }
    if (fd == 0) return ret(r, -EBADF);
    return ret(r, mosaico_vfs_write(fd, buf, n));
}

_off_t _lseek_r(struct _reent *r, int fd, _off_t off, int whence)
{
    if (fd >= 0 && fd < 3) return ret(r, -ESPIPE);
    int64_t p = mosaico_vfs_lseek(fd, off, whence);
    if (p < 0) return ret(r, (int)p);
    return (_off_t)p;
}

int _fstat_r(struct _reent *r, int fd, struct stat *st)
{
    if (fd >= 0 && fd < 3) {
        memset(st, 0, sizeof(*st));
        st->st_mode = S_IFCHR | 0666;
        return 0;
    }
    int kind = 0;
    uint32_t size = 0;
    int v = mosaico_vfs_fstat(fd, &kind, &size);
    if (v < 0) return ret(r, v);
    fill_stat(st, kind, size);
    return 0;
}

int _isatty_r(struct _reent *r, int fd)
{
    if (fd >= 0 && fd < 3) return 1;
    if (r) r->_errno = ENOTTY;
    return 0;
}

int isatty(int fd) { return _isatty_r(_REENT, fd); }

/* No symlinks: lstat is stat. */
int _stat_r(struct _reent *r, const char *path, struct stat *st);
int lstat(const char *path, struct stat *st) { return _stat_r(_REENT, path, st); }

int _stat_r(struct _reent *r, const char *path, struct stat *st)
{
    int kind = 0;
    uint32_t size = 0;
    int v = mosaico_vfs_stat(path, &kind, &size);
    if (v < 0) return ret(r, v);
    fill_stat(st, kind, size);
    return 0;
}

int _unlink_r(struct _reent *r, const char *path) { return ret(r, mosaico_vfs_unlink(path)); }
int _rename_r(struct _reent *r, const char *a, const char *b) { return ret(r, mosaico_vfs_rename(a, b)); }
int _link_r(struct _reent *r, const char *a, const char *b)
{
    (void)a;
    (void)b;
    return ret(r, -ENOSYS);
}

int _mkdir_r(struct _reent *r, const char *path, int mode)
{
    (void)mode;
    return ret(r, mosaico_vfs_mkdir(path));
}
int mkdir(const char *path, mode_t mode) { return _mkdir_r(_REENT, path, (int)mode); }
int rmdir(const char *path) { return ret(_REENT, mosaico_vfs_rmdir(path)); }
int fsync(int fd) { return ret(_REENT, mosaico_vfs_fsync(fd)); }
int ftruncate(int fd, off_t size)
{
    if (size < 0) return ret(_REENT, -EINVAL);
    return ret(_REENT, mosaico_vfs_ftruncate(fd, (uint32_t)size));
}
int truncate(const char *path, off_t size)
{
    int fd = mosaico_vfs_open(path, 2);
    if (fd < 0) return ret(_REENT, fd);
    int v = size < 0 ? -EINVAL : mosaico_vfs_ftruncate(fd, (uint32_t)size);
    mosaico_vfs_close(fd);
    return ret(_REENT, v);
}
int access(const char *path, int mode)
{
    (void)mode;
    int kind = 0;
    uint32_t size = 0;
    return ret(_REENT, mosaico_vfs_stat(path, &kind, &size));
}

int statvfs(const char *path, struct statvfs *out)
{
    uint32_t bsize = 0, blocks = 0, bfree = 0;
    int v = mosaico_vfs_statvfs(path, &bsize, &blocks, &bfree);
    if (v < 0) return ret(_REENT, v);
    memset(out, 0, sizeof(*out));
    out->f_bsize = bsize;
    out->f_frsize = bsize;
    out->f_blocks = blocks;
    out->f_bfree = bfree;
    out->f_bavail = bfree;
    out->f_namemax = 255;
    return 0;
}

/* DIR: newlib declares it opaque beyond a VFS index; this layout adds the
 * handle and the entry readdir returns. */
typedef struct {
    DIR base;
    int handle;
    struct dirent entry;
} mosaico_dir_t;

DIR *opendir(const char *path)
{
    int h = mosaico_vfs_opendir(path);
    if (h < 0) {
        errno = -h;
        return NULL;
    }
    mosaico_dir_t *d = calloc(1, sizeof(*d));
    if (!d) {
        mosaico_vfs_closedir(h);
        errno = ENOMEM;
        return NULL;
    }
    d->handle = h;
    return &d->base;
}

struct dirent *readdir(DIR *dir)
{
    mosaico_dir_t *d = (mosaico_dir_t *)dir;
    int kind = 0;
    if (!d) {
        errno = EBADF;
        return NULL;
    }
    int v = mosaico_vfs_readdir(d->handle, d->entry.d_name, &kind);
    if (v <= 0) {
        if (v < 0) errno = -v;
        return NULL;
    }
    d->entry.d_ino = 0;
    d->entry.d_type = kind == 2 ? DT_DIR : DT_REG;
    return &d->entry;
}

int closedir(DIR *dir)
{
    mosaico_dir_t *d = (mosaico_dir_t *)dir;
    if (!d) return ret(_REENT, -EBADF);
    int v = mosaico_vfs_closedir(d->handle);
    free(d);
    return ret(_REENT, v);
}
