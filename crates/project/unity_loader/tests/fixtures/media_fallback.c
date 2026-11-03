// INPUT:  bare Android NDK media functions/data globals supplied by the optional plugin
// OUTPUT:  rudroid_media_probe verifies permanent failure, untouched outputs and caller ownership
// POS:    actual ARM64 guest relocations; does not claim video decoding or game startup
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
typedef struct Extractor Extractor;
typedef struct DataSource DataSource;
typedef struct Codec Codec;
typedef struct Format Format;
typedef struct { int32_t offset,size; int64_t time; uint32_t flags; } Info;
extern const char *AMEDIAFORMAT_KEY_CHANNEL_COUNT,*AMEDIAFORMAT_KEY_COLOR_FORMAT;
extern const char *AMEDIAFORMAT_KEY_DURATION,*AMEDIAFORMAT_KEY_FRAME_RATE;
extern const char *AMEDIAFORMAT_KEY_HEIGHT,*AMEDIAFORMAT_KEY_LANGUAGE,*AMEDIAFORMAT_KEY_MIME;
extern const char *AMEDIAFORMAT_KEY_SAMPLE_RATE,*AMEDIAFORMAT_KEY_STRIDE,*AMEDIAFORMAT_KEY_WIDTH;
extern const char *AMEDIAFORMAT_KEY_COLOR_RANGE,*AMEDIAFORMAT_KEY_COLOR_STANDARD;
extern const char *AMEDIAFORMAT_KEY_ENCODER_DELAY,*AMEDIAFORMAT_KEY_ROTATION,*AMEDIAFORMAT_KEY_SLICE_HEIGHT;
extern Extractor *AMediaExtractor_new(void);
extern int32_t AMediaExtractor_delete(Extractor *);
extern int32_t AMediaExtractor_setDataSource(Extractor *,const char *);
extern int32_t AMediaExtractor_setDataSourceFd(Extractor *,int,int64_t,int64_t);
extern int32_t AMediaExtractor_setDataSourceCustom(Extractor *,DataSource *);
extern size_t AMediaExtractor_getTrackCount(Extractor *);
extern Format *AMediaExtractor_getTrackFormat(Extractor *,size_t);
extern int32_t AMediaExtractor_selectTrack(Extractor *,size_t);
extern ssize_t AMediaExtractor_readSampleData(Extractor *,uint8_t *,size_t);
extern int AMediaExtractor_getSampleTrackIndex(Extractor *);
extern int64_t AMediaExtractor_getSampleTime(Extractor *);
extern bool AMediaExtractor_advance(Extractor *);
extern int32_t AMediaExtractor_seekTo(Extractor *,int64_t,int);
extern DataSource *AMediaDataSource_new(void);
extern void AMediaDataSource_delete(DataSource *);
extern void AMediaDataSource_setUserdata(DataSource *,void *);
extern void AMediaDataSource_setReadAt(DataSource *,ssize_t (*)(void *,int64_t,void *,size_t));
extern void AMediaDataSource_setGetSize(DataSource *,ssize_t (*)(void *));
extern void AMediaDataSource_setClose(DataSource *,void (*)(void *));
extern Codec *AMediaCodec_createDecoderByType(const char *);
extern int32_t AMediaCodec_delete(Codec *);
extern int32_t AMediaCodec_configure(Codec *,const Format *,void *,void *,uint32_t);
extern int32_t AMediaCodec_start(Codec *),AMediaCodec_stop(Codec *),AMediaCodec_flush(Codec *);
extern ssize_t AMediaCodec_dequeueInputBuffer(Codec *,int64_t);
extern ssize_t AMediaCodec_dequeueOutputBuffer(Codec *,Info *,int64_t);
extern uint8_t *AMediaCodec_getInputBuffer(Codec *,size_t,size_t *);
extern uint8_t *AMediaCodec_getOutputBuffer(Codec *,size_t,size_t *);
extern int32_t AMediaCodec_queueInputBuffer(Codec *,size_t,off_t,size_t,uint64_t,uint32_t);
extern int32_t AMediaCodec_releaseOutputBuffer(Codec *,size_t,bool);
extern int32_t AMediaCodec_setOutputSurface(Codec *,void *);
extern Format *AMediaCodec_getOutputFormat(Codec *);
extern int32_t AMediaFormat_delete(Format *);
extern bool AMediaFormat_getFloat(Format *,const char *,float *);
extern bool AMediaFormat_getInt32(Format *,const char *,int32_t *);
extern bool AMediaFormat_getInt64(Format *,const char *,int64_t *);
extern bool AMediaFormat_getString(Format *,const char *,const char **);
extern void AMediaFormat_setInt32(Format *,const char *,int32_t);
#define CHECK(x) do { if (!(x)) return __LINE__; } while (0)
static int calls;
static ssize_t read_at(void *user,int64_t offset,void *buffer,size_t size) { ++calls; return -1; }
static ssize_t get_size(void *user) { ++calls; return 0; }
static void close_source(void *user) { ++calls; }
static bool equal(const char *a,const char *b) {
    while (*a && *a==*b) { ++a; ++b; } return *a==*b;
}
int rudroid_media_probe(int fd) {
    const char *keys[]={AMEDIAFORMAT_KEY_CHANNEL_COUNT,AMEDIAFORMAT_KEY_COLOR_FORMAT,
        AMEDIAFORMAT_KEY_DURATION,AMEDIAFORMAT_KEY_FRAME_RATE,AMEDIAFORMAT_KEY_HEIGHT,
        AMEDIAFORMAT_KEY_LANGUAGE,AMEDIAFORMAT_KEY_MIME,AMEDIAFORMAT_KEY_SAMPLE_RATE,
        AMEDIAFORMAT_KEY_STRIDE,AMEDIAFORMAT_KEY_WIDTH,AMEDIAFORMAT_KEY_COLOR_RANGE,
        AMEDIAFORMAT_KEY_COLOR_STANDARD,AMEDIAFORMAT_KEY_ENCODER_DELAY,
        AMEDIAFORMAT_KEY_ROTATION,AMEDIAFORMAT_KEY_SLICE_HEIGHT};
    const char *expected[]={"channel-count","color-format","durationUs","frame-rate","height",
        "language","mime","sample-rate","stride","width","color-range","color-standard",
        "encoder-delay","rotation-degrees","slice-height"};
    for (unsigned i=0;i<15;++i) CHECK(equal(keys[i],expected[i]));
    Extractor *ex=AMediaExtractor_new(); DataSource *source=AMediaDataSource_new();
    CHECK(ex && source);
    AMediaDataSource_setUserdata(source,&calls);
    AMediaDataSource_setReadAt(source,read_at); AMediaDataSource_setGetSize(source,get_size);
    AMediaDataSource_setClose(source,close_source);
    CHECK(AMediaExtractor_setDataSource(ex,"unused-video.mp4")==-10002);
    CHECK(AMediaExtractor_setDataSourceFd(ex,fd,0,8)==-10002);
    CHECK(AMediaExtractor_setDataSourceCustom(ex,source)==-10002);
    CHECK(!AMediaExtractor_getTrackCount(ex) && !AMediaExtractor_getTrackFormat(ex,0));
    CHECK(AMediaExtractor_selectTrack(ex,0)==-10002);
    uint8_t buffer[]={0x12,0x34};
    CHECK(AMediaExtractor_readSampleData(ex,buffer,2)==-1 && buffer[0]==0x12 && buffer[1]==0x34);
    CHECK(AMediaExtractor_getSampleTrackIndex(ex)==-1 && AMediaExtractor_getSampleTime(ex)==-1);
    CHECK(!AMediaExtractor_advance(ex) && AMediaExtractor_seekTo(ex,0,0)==-10002);
    CHECK(AMediaExtractor_delete(ex)==0); AMediaDataSource_delete(source);
    CHECK(calls==0); // no fd/stream/callback was adopted by failed setup.
    Codec *codec=AMediaCodec_createDecoderByType("video/avc"); CHECK(!codec);
    CHECK(AMediaCodec_configure(codec,0,0,0,0)==-10003);
    CHECK(AMediaCodec_start(codec)==-10003 && AMediaCodec_stop(codec)==-10003);
    CHECK(AMediaCodec_flush(codec)==-10003 && AMediaCodec_dequeueInputBuffer(codec,-1)==-10003);
    Info info={7,8,9,10};
    CHECK(AMediaCodec_dequeueOutputBuffer(codec,&info,-1)==-10003 && info.time==9 && info.flags==10);
    size_t size=123; CHECK(!AMediaCodec_getInputBuffer(codec,0,&size) && !size);
    size=123; CHECK(!AMediaCodec_getOutputBuffer(codec,0,&size) && !size);
    CHECK(AMediaCodec_queueInputBuffer(codec,0,0,0,0,0)==-10003);
    CHECK(AMediaCodec_releaseOutputBuffer(codec,0,false)==-10003);
    CHECK(AMediaCodec_setOutputSurface(codec,0)==-10003 && !AMediaCodec_getOutputFormat(codec));
    CHECK(AMediaCodec_delete(codec)==0);
    float f=1.25f; int32_t i=123; int64_t l=INT64_C(0x123456789); const char *str="sentinel";
    CHECK(!AMediaFormat_getFloat(0,keys[3],&f) && f==1.25f);
    CHECK(!AMediaFormat_getInt32(0,keys[4],&i) && i==123);
    CHECK(!AMediaFormat_getInt64(0,keys[2],&l) && l==INT64_C(0x123456789));
    CHECK(!AMediaFormat_getString(0,keys[6],&str) && equal(str,"sentinel"));
    AMediaFormat_setInt32(0,keys[4],123); CHECK(AMediaFormat_delete(0)==0);
    return 0;
}
