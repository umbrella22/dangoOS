/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
/* Clears any preceding handoff, then paints a static image before validation. */
void dg_display_begin(void);
/* Commit only a completed frame, just before the selected image is loaded. */
void dg_display_handoff(void);
