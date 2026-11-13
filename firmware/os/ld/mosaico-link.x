/* esp-hal bb239a0f4adc ESP32-S31 linker layout. Local six-MiB NOR budget. */
INCLUDE "mosaico-memory.x"

/* esp-hal region aliases */
REGION_ALIAS("ROTEXT", ROM);
REGION_ALIAS("RODATA", ROM);

REGION_ALIAS("RWTEXT", RAM);
REGION_ALIAS("RWDATA", RAM);

REGION_ALIAS("RTC_FAST_RWTEXT", RTC_FAST);
REGION_ALIAS("RTC_FAST_RWDATA", RTC_FAST);

/* riscv-rt v0.16+ region aliases (required by upstream riscv-rt link.x) */
REGION_ALIAS("REGION_TEXT", ROM);
REGION_ALIAS("REGION_RODATA", ROM);
REGION_ALIAS("REGION_DATA", RAM);
REGION_ALIAS("REGION_BSS", RAM);
REGION_ALIAS("REGION_HEAP", RAM);
REGION_ALIAS("REGION_STACK", RAM);

INCLUDE "esp32s31.x"
INCLUDE "hal-defaults.x"

/* The main thread and its interrupt executors share this stack. A successful
   link with only a few KiB left can otherwise fail at the first context switch. */
ASSERT(_stack_start_cpu0 - _stack_end_cpu0 >= 32K,
       "Mosaico requires at least 32 KiB for the main/interrupt stack");
