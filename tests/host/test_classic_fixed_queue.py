"""Exercise real Bluedroid queue/HCI code at a producer preemption boundary."""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
VENDOR = ROOT / "vendor/esp-bluedroid/upstream"


def function(source, name):
    start = source.index("static void " + name + "(void *arg)\n{")
    body = source.index("{", start)
    depth, end = 1, body + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


class ClassicFixedQueueTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("cc"), "native C compiler required")
    def test_preempted_producer_and_hci_repost_make_progress(self):
        with tempfile.TemporaryDirectory(prefix="mosaico-fixed-queue-") as tmp:
            tmp = pathlib.Path(tmp)
            (tmp / "osi").mkdir()
            headers = {
                "bt_common.h": "#include <assert.h>\n#define OSI_TRACE_ERROR(...) ((void)0)\n",
                "esp_assume.h": "#include <assert.h>\n#define ESP_ASSUME_NONNULL(p) assert((p) != NULL)\n",
                "osi/allocator.h": "#include <stdlib.h>\n#define osi_malloc malloc\n#define osi_calloc(n) calloc(1, (n))\n#define osi_free free\n",
                "osi/mutex.h": """#pragma once
#include <stdint.h>
typedef struct host_mutex *osi_mutex_t;
#define OSI_MUTEX_MAX_TIMEOUT UINT32_MAX
int osi_mutex_new(osi_mutex_t *);
int osi_mutex_lock(osi_mutex_t *, uint32_t);
void osi_mutex_unlock(osi_mutex_t *);
void osi_mutex_free(osi_mutex_t *);
""",
                "osi/semaphore.h": """#pragma once
#include <stdint.h>
typedef struct host_sem *osi_sem_t;
#define OSI_SEM_MAX_TIMEOUT UINT32_MAX
int osi_sem_new(osi_sem_t *, uint32_t, uint32_t);
int osi_sem_take(osi_sem_t *, uint32_t);
void osi_sem_give(osi_sem_t *);
void osi_sem_free(osi_sem_t *);
""",
            }
            for name, text in headers.items():
                (tmp / name).write_text(text)
            handler = tmp / "hci_handler.c"
            handler.write_text(function(
                (VENDOR / "host/bluedroid/hci/hci_hal_h4.c").read_text(),
                "hci_upstream_data_handler"))
            binary = tmp / "fixed-queue"
            command = [
                "cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                "-Wno-unused-parameter", "-I", str(tmp),
                "-I", str(VENDOR / "common/osi/include"),
                '-DFIXED_QUEUE_SOURCE="' + str(VENDOR / "common/osi/fixed_queue.c") + '"',
                '-DHCI_HANDLER_SOURCE="' + str(handler) + '"',
                str(ROOT / "tests/host/fixtures/classic_fixed_queue.c"),
                str(VENDOR / "common/osi/list.c"), "-o", str(binary),
            ]
            build = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("producer preemption and HCI receive progress passed", result.stdout)
