#pragma once
/* Software ladder; hardware ECC belongs to the Rust HAL owner. */
#undef SOC_ECC_SUPPORTED
#define SOC_ECC_SUPPORTED 0
