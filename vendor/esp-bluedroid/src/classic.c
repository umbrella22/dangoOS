// SPDX-License-Identifier: Apache-2.0
#include <stdatomic.h>
#include <string.h>
#include "esp_bluedroid_hci.h"
#include "esp_bt_main.h"
#include "esp_bt_device.h"
#include "esp_gap_bt_api.h"
#include "esp_a2dp_api.h"
#include "esp_avrc_api.h"
#include "freertos/FreeRTOS.h"

extern void mosaico_bt_hci_send(uint8_t*,uint16_t);
extern bool mosaico_bt_hci_available(void);
extern void mosaico_bt_event(unsigned,unsigned,unsigned);
extern void mosaico_bt_pcm(const uint8_t*,uint32_t);
extern void mosaico_bt_remote_volume(unsigned);
static esp_bluedroid_hci_driver_callbacks_t callbacks;
static atomic_int profile_state;
static atomic_int ct_state, tg_state;
static bool sink_requested,ct_started,tg_started;
static atomic_bool stopping;
static atomic_uint current_volume;
static atomic_bool volume_registered;
static atomic_uint volume_handle;
static void volume_response(esp_a2d_conn_hdl_t handle,esp_avrc_rn_rsp_t response,unsigned value){
    esp_avrc_rn_param_t param={.volume=value};
    esp_avrc_tg_send_rn_rsp_by_handle(handle,ESP_AVRC_RN_VOLUME_CHANGE,response,&param);
}
static void volume_changed(void){
    if(atomic_exchange(&volume_registered,false))
        volume_response(atomic_load(&volume_handle),ESP_AVRC_RN_RSP_CHANGED,atomic_load(&current_volume));
}
void mosaico_classic_volume(unsigned percent){
    unsigned value=(percent*127U+50U)/100U;
    if(value>127)value=127;
    if(atomic_exchange(&current_volume,value)!=value)volume_changed();
}
static int register_callbacks(const esp_bluedroid_hci_driver_callbacks_t *cb){callbacks=*cb;return ESP_OK;}
int mosaico_classic_receive(uint8_t *data,uint16_t len){return callbacks.notify_host_recv?callbacks.notify_host_recv(data,len):-1;}
void mosaico_classic_send_available(void){if(callbacks.notify_host_send_available)callbacks.notify_host_send_available();}
bool mosaico_classic_host_uninitialized(void){return esp_bluedroid_get_status()==ESP_BLUEDROID_STATUS_UNINITIALIZED;}
static void gap(esp_bt_gap_cb_event_t event,esp_bt_gap_cb_param_t *p){
    switch(event){
    case ESP_BT_GAP_CFM_REQ_EVT:
        esp_bt_gap_ssp_confirm_reply(p->cfm_req.bda,!atomic_load(&stopping));break;
    case ESP_BT_GAP_PIN_REQ_EVT:
        // This audio service uses SSP Just Works; it does not silently choose a legacy PIN.
        esp_bt_gap_pin_reply(p->pin_req.bda,false,0,NULL);break;
    case ESP_BT_GAP_AUTH_CMPL_EVT:
        mosaico_bt_event(4,p->auth_cmpl.stat,0);break;
    default:break;
    }
}
static void a2dp(esp_a2d_cb_event_t event,esp_a2d_cb_param_t *p){
    if(event==ESP_A2D_PROF_STATE_EVT){atomic_store(&profile_state,p->a2d_prof_stat.init_state==ESP_A2D_INIT_SUCCESS?1:0);return;}
    if(atomic_load(&stopping))return;
    switch(event){
    case ESP_A2D_CONNECTION_STATE_EVT:
        mosaico_bt_event(1,p->conn_stat.state,p->conn_stat.disc_rsn);break;
    case ESP_A2D_AUDIO_STATE_EVT:
        mosaico_bt_event(2,p->audio_stat.state,0);break;
    case ESP_A2D_AUDIO_CFG_EVT:{
        if(p->audio_cfg.mcc.type!=ESP_A2D_MCT_SBC){mosaico_bt_event(3,0,0);break;}
        uint8_t b=*(const uint8_t *)&p->audio_cfg.mcc.cie.sbc_info;
        unsigned rate=(b&0x80)?16000:(b&0x40)?32000:(b&0x20)?44100:(b&0x10)?48000:0;
        mosaico_bt_event(3,rate,(b&0x08)?1:2);break;
    }
    default:break;
    }
}
static void pcm(const uint8_t *data,uint32_t len){if(!atomic_load(&stopping))mosaico_bt_pcm(data,len);}
static void avrc_ct(esp_avrc_ct_cb_event_t e,esp_avrc_ct_cb_param_t *p){
    if(e==ESP_AVRC_CT_PROF_STATE_EVT)atomic_store(&ct_state,p->avrc_ct_init_stat.state);
}
static void avrc_tg(esp_avrc_tg_cb_event_t e,esp_avrc_tg_cb_param_t *p){
    // Lifecycle acknowledgements remain necessary while shutdown suppresses
    // ordinary peer events and audio callbacks.
    if(e==ESP_AVRC_TG_PROF_STATE_EVT){atomic_store(&tg_state,p->avrc_tg_init_stat.state);return;}
    if(atomic_load(&stopping))return;
    if(e==ESP_AVRC_TG_CONNECTION_STATE_EVT&&!p->conn_stat.connected)atomic_store(&volume_registered,false);
    if(e==ESP_AVRC_TG_SET_ABSOLUTE_VOLUME_CMD_EVT){
        atomic_store(&current_volume,p->set_abs_vol.volume);
        unsigned percent=(p->set_abs_vol.volume*100U+63U)/127U;
        mosaico_bt_remote_volume(percent);
        mosaico_bt_event(5,p->set_abs_vol.volume,percent);
    }
    if(e==ESP_AVRC_TG_REGISTER_NOTIFICATION_EVT&&p->reg_ntf.event_id==ESP_AVRC_RN_VOLUME_CHANGE){
        unsigned value=atomic_load(&current_volume);
        atomic_store(&volume_handle,p->reg_ntf.conn_hdl);
        volume_response(p->reg_ntf.conn_hdl,ESP_AVRC_RN_RSP_INTERIM,value);
        atomic_store(&volume_registered,true);
        if(atomic_load(&current_volume)!=value)volume_changed();
    }
}
static int wait_profile(int state){
    uint32_t start=xTaskGetTickCount();
    while(atomic_load(&profile_state)!=state){if(xTaskGetTickCount()-start>6000)return ESP_ERR_TIMEOUT;vTaskDelay(2);}
    return ESP_OK;
}
static int wait_avrc(atomic_int *state,int expected){
    uint32_t start=xTaskGetTickCount();
    for(;;){
        int value=atomic_load(state);
        if(value==expected || (expected==ESP_AVRC_DEINIT_SUCCESS && value==ESP_AVRC_DEINIT_ALREADY))return ESP_OK;
        if(value!=-1)return ESP_FAIL;
        if(xTaskGetTickCount()-start>6000)return ESP_ERR_TIMEOUT;
        vTaskDelay(2);
    }
}
int mosaico_classic_stop(void){
    atomic_store(&stopping,true);
    atomic_store(&volume_registered,false);
    if(esp_bluedroid_get_status()==ESP_BLUEDROID_STATUS_ENABLED){
        esp_bt_gap_set_scan_mode(ESP_BT_NON_CONNECTABLE,ESP_BT_NON_DISCOVERABLE);
        // AVRCP shares A2DP control state. Its asynchronous deinit must finish
        // before A2DP frees that state (the host warns about the reverse order).
        if(ct_started){
            atomic_store(&ct_state,-1);
            int rc=esp_avrc_ct_deinit();if(rc!=ESP_OK)return rc;
            rc=wait_avrc(&ct_state,ESP_AVRC_DEINIT_SUCCESS);if(rc!=ESP_OK)return rc;
            ct_started=false;
        }
        if(tg_started){
            atomic_store(&tg_state,-1);
            int rc=esp_avrc_tg_deinit();if(rc!=ESP_OK)return rc;
            rc=wait_avrc(&tg_state,ESP_AVRC_DEINIT_SUCCESS);if(rc!=ESP_OK)return rc;
            tg_started=false;
        }
        if(sink_requested){
            int rc=esp_a2d_sink_deinit();if(rc!=ESP_OK)return rc;
            rc=wait_profile(0);if(rc!=ESP_OK)return rc;sink_requested=false;
        }
        int rc=esp_bluedroid_disable();if(rc!=ESP_OK)return rc;
    }
    if(esp_bluedroid_get_status()==ESP_BLUEDROID_STATUS_INITIALIZED){int rc=esp_bluedroid_deinit();if(rc!=ESP_OK)return rc;}
    if(!mosaico_classic_host_uninitialized())return ESP_ERR_INVALID_STATE;
    memset(&callbacks,0,sizeof(callbacks));int rc=esp_bluedroid_detach_hci_driver();return rc;
}
int mosaico_classic_start(void){
    atomic_store(&stopping,false);atomic_store(&profile_state,-1);
    atomic_store(&ct_state,-1);atomic_store(&tg_state,-1);
    const esp_bluedroid_hci_driver_operations_t ops={.send=mosaico_bt_hci_send,.check_send_available=mosaico_bt_hci_available,.register_host_callback=register_callbacks};
    int rc=esp_bluedroid_attach_hci_driver(&ops);if(rc!=ESP_OK)return rc;
#define TRY(call) do{rc=(call);if(rc!=ESP_OK)goto fail;}while(0)
    TRY(esp_bluedroid_init());
    TRY(esp_bluedroid_enable());
    TRY(esp_bt_gap_register_callback(gap));
    TRY(esp_bt_gap_set_device_name("ESP-Mosaico Audio"));
    esp_bt_io_cap_t io=ESP_BT_IO_CAP_NONE;
    TRY(esp_bt_gap_set_security_param(ESP_BT_SP_IOCAP_MODE,&io,sizeof(io)));
    esp_bt_cod_t cod={.major=ESP_BT_COD_MAJOR_DEV_AV,.minor=5,.service=ESP_BT_COD_SRVC_AUDIO|ESP_BT_COD_SRVC_RENDERING};
    TRY(esp_bt_gap_set_cod(cod,ESP_BT_SET_COD_ALL));
    TRY(esp_avrc_ct_register_callback(avrc_ct));
    TRY(esp_avrc_ct_init());ct_started=true;
    TRY(wait_avrc(&ct_state,ESP_AVRC_INIT_SUCCESS));
    TRY(esp_avrc_tg_register_callback(avrc_tg));
    TRY(esp_avrc_tg_init());tg_started=true;
    TRY(wait_avrc(&tg_state,ESP_AVRC_INIT_SUCCESS));
    esp_avrc_rn_evt_cap_mask_t notifications={.bits=1U<<ESP_AVRC_RN_VOLUME_CHANGE};
    TRY(esp_avrc_tg_set_rn_evt_cap(&notifications));
    TRY(esp_a2d_register_callback(a2dp));
    TRY(esp_a2d_sink_register_data_callback(pcm));
    TRY(esp_a2d_sink_init());sink_requested=true;
    TRY(wait_profile(1));
    TRY(esp_bt_gap_set_scan_mode(ESP_BT_CONNECTABLE,ESP_BT_GENERAL_DISCOVERABLE));
    return ESP_OK;
fail:
    mosaico_classic_stop();return rc;
}
