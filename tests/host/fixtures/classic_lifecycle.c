// The real adapter is compiled with the imported API headers. Only the host
// APIs and scheduler are replaced; callbacks are delivered after API return.
#include <assert.h>
#include <stdio.h>
#include "classic.c"

static esp_bluedroid_status_t host;
static esp_avrc_ct_cb_t ct_cb;
static esp_avrc_tg_cb_t tg_cb;
static esp_a2d_cb_t sink_cb;
static esp_bt_gap_cb_t gap_cb;
static bool attached, ct_active, tg_active, sink_active;
static bool fail_tg_init, fail_ct_stop, lose_ct_stop;
static unsigned pending_ct, pending_tg, pending_sink;
static uint32_t ticks;
static unsigned interim, changed, remote_percent, accepted, deinit_count;
static esp_avrc_rn_rsp_t last_response;
static unsigned last_volume, last_handle, last_phase;

void mosaico_bt_task_delay(uint32_t ms) {
    ticks += ms;
    if (pending_ct) {
        esp_avrc_ct_cb_param_t p = {0};
        p.avrc_ct_init_stat.state = pending_ct - 1;
        ct_active = p.avrc_ct_init_stat.state == ESP_AVRC_INIT_SUCCESS;
        pending_ct = 0;
        ct_cb(ESP_AVRC_CT_PROF_STATE_EVT, &p);
    }
    if (pending_tg) {
        esp_avrc_tg_cb_param_t p = {0};
        p.avrc_tg_init_stat.state = pending_tg - 1;
        tg_active = p.avrc_tg_init_stat.state == ESP_AVRC_INIT_SUCCESS;
        pending_tg = 0;
        tg_cb(ESP_AVRC_TG_PROF_STATE_EVT, &p);
    }
    if (pending_sink) {
        esp_a2d_cb_param_t p = {0};
        p.a2d_prof_stat.init_state = pending_sink - 1;
        sink_active = p.a2d_prof_stat.init_state == ESP_A2D_INIT_SUCCESS;
        pending_sink = 0;
        sink_cb(ESP_A2D_PROF_STATE_EVT, &p);
    }
}
uint32_t mosaico_bt_ticks(void) { return ticks; }
void mosaico_bt_phase(unsigned phase) { last_phase = phase; }
void mosaico_bt_hci_send(uint8_t *p, uint16_t len) { (void)p; (void)len; }
bool mosaico_bt_hci_available(void) { return true; }
void mosaico_bt_event(unsigned kind, unsigned a, unsigned b) { (void)kind; (void)a; (void)b; }
void mosaico_bt_pcm(const uint8_t *p, uint32_t len) { (void)p; (void)len; }
void mosaico_bt_remote_volume(unsigned percent) { remote_percent = percent; }

esp_bluedroid_status_t esp_bluedroid_get_status(void) { return host; }
esp_err_t esp_bluedroid_attach_hci_driver(const esp_bluedroid_hci_driver_operations_t *ops) {
    assert(!attached && host == ESP_BLUEDROID_STATUS_UNINITIALIZED);
    assert(ops->register_host_callback && ops->send && ops->check_send_available);
    attached = true;
    return ESP_OK;
}
esp_err_t esp_bluedroid_detach_hci_driver(void) {
    assert(attached && host == ESP_BLUEDROID_STATUS_UNINITIALIZED);
    attached = false;
    return ESP_OK;
}
esp_err_t esp_bluedroid_init(void) {
    assert(attached && host == ESP_BLUEDROID_STATUS_UNINITIALIZED);
    host = ESP_BLUEDROID_STATUS_INITIALIZED;
    return ESP_OK;
}
esp_err_t esp_bluedroid_enable(void) {
    assert(host == ESP_BLUEDROID_STATUS_INITIALIZED);
    host = ESP_BLUEDROID_STATUS_ENABLED;
    return ESP_OK;
}
esp_err_t esp_bluedroid_disable(void) {
    assert(host == ESP_BLUEDROID_STATUS_ENABLED);
    assert(!ct_active && !tg_active && !sink_active);
    host = ESP_BLUEDROID_STATUS_INITIALIZED;
    return ESP_OK;
}
esp_err_t esp_bluedroid_deinit(void) {
    assert(host == ESP_BLUEDROID_STATUS_INITIALIZED);
    host = ESP_BLUEDROID_STATUS_UNINITIALIZED;
    ++deinit_count;
    return ESP_OK;
}
esp_err_t esp_bt_gap_register_callback(esp_bt_gap_cb_t cb) { gap_cb = cb; return ESP_OK; }
esp_err_t esp_bt_gap_set_device_name(const char *name) { assert(!strcmp(name, "ESP-Mosaico Audio")); return ESP_OK; }
esp_err_t esp_bt_gap_set_security_param(esp_bt_sp_param_t type, void *v, uint8_t len) {
    assert(type == ESP_BT_SP_IOCAP_MODE && len == sizeof(esp_bt_io_cap_t));
    assert(*(esp_bt_io_cap_t *)v == ESP_BT_IO_CAP_NONE);
    return ESP_OK;
}
esp_err_t esp_bt_gap_set_cod(esp_bt_cod_t cod, esp_bt_cod_mode_t mode) { (void)cod; (void)mode; return ESP_OK; }
esp_err_t esp_bt_gap_set_scan_mode(esp_bt_connection_mode_t c, esp_bt_discovery_mode_t d) { (void)c; (void)d; return ESP_OK; }
esp_err_t esp_bt_gap_ssp_confirm_reply(esp_bd_addr_t addr, bool allow) { (void)addr; accepted = allow; return ESP_OK; }
esp_err_t esp_bt_gap_pin_reply(esp_bd_addr_t addr, bool allow, uint8_t len, esp_bt_pin_code_t pin) {
    (void)addr; (void)pin; assert(!allow && !len); return ESP_OK;
}
esp_err_t esp_avrc_ct_register_callback(esp_avrc_ct_cb_t cb) { ct_cb = cb; return ESP_OK; }
esp_err_t esp_avrc_tg_register_callback(esp_avrc_tg_cb_t cb) { tg_cb = cb; return ESP_OK; }
esp_err_t esp_avrc_ct_init(void) { assert(ct_cb); pending_ct = ESP_AVRC_INIT_SUCCESS + 1; return ESP_OK; }
esp_err_t esp_avrc_tg_init(void) {
    assert(tg_cb && ct_active);
    pending_tg = (fail_tg_init ? ESP_AVRC_INIT_FAIL : ESP_AVRC_INIT_SUCCESS) + 1;
    return ESP_OK;
}
esp_err_t esp_avrc_ct_deinit(void) {
    if (fail_ct_stop) return ESP_FAIL;
    if (!lose_ct_stop) pending_ct = ESP_AVRC_DEINIT_SUCCESS + 1;
    return ESP_OK;
}
esp_err_t esp_avrc_tg_deinit(void) { pending_tg = (tg_active ? ESP_AVRC_DEINIT_SUCCESS : ESP_AVRC_DEINIT_ALREADY) + 1; return ESP_OK; }
esp_err_t esp_avrc_tg_set_rn_evt_cap(const esp_avrc_rn_evt_cap_mask_t *cap) {
    assert(tg_active && (cap->bits & (1U << ESP_AVRC_RN_VOLUME_CHANGE))); return ESP_OK;
}
esp_err_t esp_avrc_tg_send_rn_rsp_by_handle(esp_a2d_conn_hdl_t h, esp_avrc_rn_event_ids_t event,
                                         esp_avrc_rn_rsp_t response, esp_avrc_rn_param_t *p) {
    assert(event == ESP_AVRC_RN_VOLUME_CHANGE);
    last_handle = h; last_response = response; last_volume = p->volume;
    if (response == ESP_AVRC_RN_RSP_INTERIM) ++interim;
    if (response == ESP_AVRC_RN_RSP_CHANGED) ++changed;
    return ESP_OK;
}
esp_err_t esp_a2d_register_callback(esp_a2d_cb_t cb) { sink_cb = cb; return ESP_OK; }
esp_err_t esp_a2d_sink_register_data_callback(esp_a2d_sink_data_cb_t cb) { assert(cb); return ESP_OK; }
esp_err_t esp_a2d_sink_init(void) {
    assert(ct_active && tg_active && sink_cb);
    pending_sink = ESP_A2D_INIT_SUCCESS + 1;
    return ESP_OK;
}
esp_err_t esp_a2d_sink_deinit(void) {
    assert(!ct_active && !tg_active);
    pending_sink = ESP_A2D_DEINIT_SUCCESS + 1;
    return ESP_OK;
}

static void volume_notifications(void) {
    mosaico_classic_volume(50);
    esp_avrc_tg_cb_param_t p = {0};
    p.reg_ntf.conn_hdl = 7;
    p.reg_ntf.event_id = ESP_AVRC_RN_VOLUME_CHANGE;
    tg_cb(ESP_AVRC_TG_REGISTER_NOTIFICATION_EVT, &p);
    assert(interim == 1 && last_volume == 64 && last_handle == 7);
    mosaico_classic_volume(70);
    assert(changed == 1 && last_response == ESP_AVRC_RN_RSP_CHANGED && last_volume == 89);
    mosaico_classic_volume(71);
    assert(changed == 1); // A notification subscription is consumed once.
    tg_cb(ESP_AVRC_TG_REGISTER_NOTIFICATION_EVT, &p);
    p.set_abs_vol.volume = 127;
    tg_cb(ESP_AVRC_TG_SET_ABSOLUTE_VOLUME_CMD_EVT, &p);
    assert(remote_percent == 100 && changed == 1);
    mosaico_classic_volume(100); // Remote writes do not cause a notification loop.
    assert(changed == 1);
    p.conn_stat.connected = false;
    tg_cb(ESP_AVRC_TG_CONNECTION_STATE_EVT, &p);
    mosaico_classic_volume(30);
    assert(changed == 1);
}

int main(void) {
    assert(mosaico_classic_start() == ESP_OK && last_phase == 100);
    assert(ct_active && tg_active && sink_active);
    esp_bt_gap_cb_param_t pairing = {0};
    gap_cb(ESP_BT_GAP_CFM_REQ_EVT, &pairing);
    assert(accepted);
    volume_notifications();
    assert(mosaico_classic_stop() == ESP_OK && last_phase == 0 && !attached);
    gap_cb(ESP_BT_GAP_CFM_REQ_EVT, &pairing);
    assert(!accepted);
    assert(mosaico_classic_start() == ESP_OK); // Recreate all profile state.
    fail_ct_stop = true;
    assert(mosaico_classic_stop() == ESP_FAIL);
    assert(attached && host == ESP_BLUEDROID_STATUS_ENABLED && sink_active);
    fail_ct_stop = false;
    lose_ct_stop = true;
    assert(mosaico_classic_stop() == ESP_ERR_TIMEOUT);
    assert(attached && host == ESP_BLUEDROID_STATUS_ENABLED && sink_active);
    lose_ct_stop = false;
    assert(mosaico_classic_stop() == ESP_OK && !attached);
    fail_tg_init = true;
    assert(mosaico_classic_start() == ESP_FAIL);
    assert(host == ESP_BLUEDROID_STATUS_UNINITIALIZED && !attached && deinit_count == 3);
    puts("asynchronous lifecycle and volume passed");
    return 0;
}
