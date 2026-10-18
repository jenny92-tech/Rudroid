// INPUT:  native C heap and fixed Android 15 NDK media prototypes/data keys
// OUTPUT:  rudroid_media_symbol for explicit unsupported decoder/extractor/format entry points
// POS:    typed failure boundary; no file adoption, decoder buffers, worker threads or Rust owners
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>

typedef struct { uint32_t reserved; } Extractor;
typedef struct { uint32_t reserved; } DataSource;
typedef struct Codec Codec;
typedef struct Format Format;
typedef struct Window Window;
typedef struct Crypto Crypto;
typedef struct BufferInfo BufferInfo;
typedef ssize_t (*ReadAt)(void *,int64_t,void *,size_t);
typedef ssize_t (*GetSize)(void *);
typedef void (*Close)(void *);
enum { OK=0, UNSUPPORTED=-10002, INVALID_OBJECT=-10003 };
_Static_assert(sizeof(void *)==8 && sizeof(off_t)==8 && sizeof(ssize_t)==8,"ARM64 LP64 media ABI");

// Each factory owns one fallible 4-byte native token. Failed source setup
// leaves fd and custom callbacks with the caller, without retaining them.
static Extractor *extractor_new(void) { return calloc(1,sizeof(Extractor)); }
static int32_t extractor_delete(Extractor *ex) { free(ex); return OK; }
static int32_t source_uri(Extractor *ex,const char *uri) { return ex ? UNSUPPORTED : INVALID_OBJECT; }
static int32_t source_fd(Extractor *ex,int fd,int64_t offset,int64_t length) { return ex ? UNSUPPORTED : INVALID_OBJECT; }
static int32_t source_custom(Extractor *ex,DataSource *source) { return ex ? UNSUPPORTED : INVALID_OBJECT; }
static size_t track_count(Extractor *ex) { return 0; }
static Format *track_format(Extractor *ex,size_t index) { return NULL; }
static int32_t select_track(Extractor *ex,size_t index) { return ex ? UNSUPPORTED : INVALID_OBJECT; }
static ssize_t read_sample(Extractor *ex,uint8_t *buffer,size_t capacity) { return -1; }
static int sample_track(Extractor *ex) { return -1; }
static int64_t sample_time(Extractor *ex) { return -1; }
static bool advance(Extractor *ex) { return false; }
static int32_t seek_to(Extractor *ex,int64_t time,int mode) { return ex ? UNSUPPORTED : INVALID_OBJECT; }
static DataSource *data_new(void) { return calloc(1,sizeof(DataSource)); }
static void data_delete(DataSource *source) { free(source); }
static void data_userdata(DataSource *source,void *user) {}
static void data_read(DataSource *source,ReadAt read) {}
static void data_size(DataSource *source,GetSize size) {}
static void data_close(DataSource *source,Close close) {}

// No codec handle can be created. Misuse returns a permanent error, not a
// transient TRY_AGAIN_LATER that might produce an unbounded retry loop.
static Codec *decoder(const char *mime) { return NULL; }
static int32_t codec_delete(Codec *codec) { return codec ? UNSUPPORTED : OK; }
static int32_t configure(Codec *codec,const Format *format,Window *surface,Crypto *crypto,uint32_t flags) { return INVALID_OBJECT; }
static int32_t codec_state(Codec *codec) { return INVALID_OBJECT; }
static ssize_t dequeue_input(Codec *codec,int64_t timeout) { return INVALID_OBJECT; }
static ssize_t dequeue_output(Codec *codec,BufferInfo *info,int64_t timeout) { return INVALID_OBJECT; }
static uint8_t *get_buffer(Codec *codec,size_t index,size_t *size) { if (size) *size=0; return NULL; }
static int32_t queue_input(Codec *codec,size_t index,off_t offset,size_t size,uint64_t time,uint32_t flags) { return INVALID_OBJECT; }
static int32_t release_output(Codec *codec,size_t index,bool render) { return INVALID_OBJECT; }
static int32_t output_surface(Codec *codec,Window *surface) { return INVALID_OBJECT; }
static Format *output_format(Codec *codec) { return NULL; }

// No format is fabricated: getters report no key, keeping caller outputs
// untouched on false, as the NDK find-value contract requires.
static int32_t format_delete(Format *format) { return format ? INVALID_OBJECT : OK; }
static bool format_float(Format *format,const char *key,float *value) { return false; }
static bool format_int32(Format *format,const char *key,int32_t *value) { return false; }
static bool format_int64(Format *format,const char *key,int64_t *value) { return false; }
static bool format_string(Format *format,const char *key,const char **value) { return false; }
static void format_set_int32(Format *format,const char *key,int32_t value) {}

static const char *KEY_CHANNEL_COUNT="channel-count";
static const char *KEY_COLOR_FORMAT="color-format";
static const char *KEY_DURATION="durationUs";
static const char *KEY_FRAME_RATE="frame-rate";
static const char *KEY_HEIGHT="height";
static const char *KEY_LANGUAGE="language";
static const char *KEY_MIME="mime";
static const char *KEY_SAMPLE_RATE="sample-rate";
static const char *KEY_STRIDE="stride";
static const char *KEY_WIDTH="width";
static const char *KEY_COLOR_RANGE="color-range";
static const char *KEY_COLOR_STANDARD="color-standard";
static const char *KEY_ENCODER_DELAY="encoder-delay";
static const char *KEY_ROTATION="rotation-degrees";
static const char *KEY_SLICE_HEIGHT="slice-height";

uintptr_t rudroid_media_symbol(const char *name) {
#define SYMBOL(n,p) if (!strcmp(name,n)) return (uintptr_t)(p)
    SYMBOL("AMEDIAFORMAT_KEY_CHANNEL_COUNT",&KEY_CHANNEL_COUNT);
    SYMBOL("AMEDIAFORMAT_KEY_COLOR_FORMAT",&KEY_COLOR_FORMAT);
    SYMBOL("AMEDIAFORMAT_KEY_DURATION",&KEY_DURATION);
    SYMBOL("AMEDIAFORMAT_KEY_FRAME_RATE",&KEY_FRAME_RATE);
    SYMBOL("AMEDIAFORMAT_KEY_HEIGHT",&KEY_HEIGHT);
    SYMBOL("AMEDIAFORMAT_KEY_LANGUAGE",&KEY_LANGUAGE);
    SYMBOL("AMEDIAFORMAT_KEY_MIME",&KEY_MIME);
    SYMBOL("AMEDIAFORMAT_KEY_SAMPLE_RATE",&KEY_SAMPLE_RATE);
    SYMBOL("AMEDIAFORMAT_KEY_STRIDE",&KEY_STRIDE);
    SYMBOL("AMEDIAFORMAT_KEY_WIDTH",&KEY_WIDTH);
    SYMBOL("AMEDIAFORMAT_KEY_COLOR_RANGE",&KEY_COLOR_RANGE);
    SYMBOL("AMEDIAFORMAT_KEY_COLOR_STANDARD",&KEY_COLOR_STANDARD);
    SYMBOL("AMEDIAFORMAT_KEY_ENCODER_DELAY",&KEY_ENCODER_DELAY);
    SYMBOL("AMEDIAFORMAT_KEY_ROTATION",&KEY_ROTATION);
    SYMBOL("AMEDIAFORMAT_KEY_SLICE_HEIGHT",&KEY_SLICE_HEIGHT);
    SYMBOL("AMediaExtractor_new",extractor_new);
    SYMBOL("AMediaExtractor_delete",extractor_delete);
    SYMBOL("AMediaExtractor_setDataSource",source_uri);
    SYMBOL("AMediaExtractor_setDataSourceFd",source_fd);
    SYMBOL("AMediaExtractor_setDataSourceCustom",source_custom);
    SYMBOL("AMediaExtractor_getTrackCount",track_count);
    SYMBOL("AMediaExtractor_getTrackFormat",track_format);
    SYMBOL("AMediaExtractor_selectTrack",select_track);
    SYMBOL("AMediaExtractor_readSampleData",read_sample);
    SYMBOL("AMediaExtractor_getSampleTrackIndex",sample_track);
    SYMBOL("AMediaExtractor_getSampleTime",sample_time);
    SYMBOL("AMediaExtractor_advance",advance);
    SYMBOL("AMediaExtractor_seekTo",seek_to);
    SYMBOL("AMediaDataSource_new",data_new);
    SYMBOL("AMediaDataSource_delete",data_delete);
    SYMBOL("AMediaDataSource_setUserdata",data_userdata);
    SYMBOL("AMediaDataSource_setReadAt",data_read);
    SYMBOL("AMediaDataSource_setGetSize",data_size);
    SYMBOL("AMediaDataSource_setClose",data_close);
    SYMBOL("AMediaCodec_createDecoderByType",decoder);
    SYMBOL("AMediaCodec_delete",codec_delete);
    SYMBOL("AMediaCodec_configure",configure);
    SYMBOL("AMediaCodec_start",codec_state);
    SYMBOL("AMediaCodec_stop",codec_state);
    SYMBOL("AMediaCodec_flush",codec_state);
    SYMBOL("AMediaCodec_dequeueInputBuffer",dequeue_input);
    SYMBOL("AMediaCodec_dequeueOutputBuffer",dequeue_output);
    SYMBOL("AMediaCodec_getInputBuffer",get_buffer);
    SYMBOL("AMediaCodec_getOutputBuffer",get_buffer);
    SYMBOL("AMediaCodec_queueInputBuffer",queue_input);
    SYMBOL("AMediaCodec_releaseOutputBuffer",release_output);
    SYMBOL("AMediaCodec_setOutputSurface",output_surface);
    SYMBOL("AMediaCodec_getOutputFormat",output_format);
    SYMBOL("AMediaFormat_delete",format_delete);
    SYMBOL("AMediaFormat_getFloat",format_float);
    SYMBOL("AMediaFormat_getInt32",format_int32);
    SYMBOL("AMediaFormat_getInt64",format_int64);
    SYMBOL("AMediaFormat_getString",format_string);
    SYMBOL("AMediaFormat_setInt32",format_set_int32);
#undef SYMBOL
    return 0;
}
