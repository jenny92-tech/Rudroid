// INPUT:  Android liblog varargs and the process-pinned HostApi log callback
// OUTPUT:  rudroid_bionic_logging_symbol/_symbol_name table and one native callback installation boundary
// POS:    formats on a bounded stack buffer and forwards to the same core log service
#define _GNU_SOURCE
#include <errno.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "symbol_table.h"
typedef void (*Logger)(const char *,const char *);
static _Atomic(Logger) logger;
static _Atomic int minimum_priority=1; // ANDROID_LOG_DEFAULT, host default INFO.
static int get_minimum(void) { return atomic_load_explicit(&minimum_priority,memory_order_relaxed); }
static int set_minimum(int priority) { return atomic_exchange_explicit(&minimum_priority,priority,memory_order_relaxed); }
static int loggable(int priority,const char *tag,int default_priority) {
    (void)tag; (void)default_priority;
    int minimum=get_minimum(); return priority >= (minimum==1 ? 4 : minimum);
}
void rudroid_bionic_install_logger(Logger next) { atomic_store_explicit(&logger,next,memory_order_release); }
static int emit(const char *tag,const char *message) {
    if (!message) return -EINVAL;
    if (!tag) tag="<null>";
    int saved=errno;
    Logger sink=atomic_load_explicit(&logger,memory_order_acquire);
    if (sink) sink(tag,message);
    else fprintf(stderr,"[%s] %s\n",tag,message); // Standalone provider before loader init.
    errno=saved; return 1;
}
static int write_log(int priority,const char *tag,const char *message) {
    if (!loggable(priority,tag,2)) return -EPERM;
    return emit(tag,message);
}
static int print_list(int priority,const char *tag,const char *format,va_list arguments) {
    if (!loggable(priority,tag,2)) return -EPERM;
    if (!format) return -EINVAL;
    int saved=errno;
    char buffer[1024]; // Android liblog printf buffer, including trailing NUL.
    int length=vsnprintf(buffer,sizeof(buffer),format,arguments);
    if (length<0) { errno=saved; return -EINVAL; }
    int result=write_log(priority,tag,buffer);
    errno=saved; return result;
}
static int print_log(int priority,const char *tag,const char *format,...) {
    va_list arguments; va_start(arguments,format);
    int result=print_list(priority,tag,format,arguments);
    va_end(arguments); return result;
}
static int buffer_write(int id,int priority,const char *tag,const char *message) {
    (void)id; return write_log(priority,tag,message);
}
static int buffer_print(int id,int priority,const char *tag,const char *format,...) {
    (void)id; va_list arguments; va_start(arguments,format);
    int result=print_list(priority,tag,format,arguments);
    va_end(arguments); return result;
}
static void assert_log(const char *condition,const char *tag,const char *format,...) {
    char buffer[1024]={0}; // Encoding errors must never expose uninitialized stack bytes.
    if (format) {
        va_list arguments; va_start(arguments,format);
        vsnprintf(buffer,sizeof(buffer),format,arguments); va_end(arguments);
    } else snprintf(buffer,sizeof(buffer),"Assertion failed: %s",condition ? condition : "");
    emit(tag,buffer); // Assertions bypass the ordinary minimum-priority filter.
    abort();
}
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("__android_log_get_minimum_priority", get_minimum)
    RUDROID_SYMBOL("__android_log_set_minimum_priority", set_minimum)
    RUDROID_SYMBOL("__android_log_is_loggable", loggable)
    RUDROID_SYMBOL("__android_log_write", write_log) RUDROID_SYMBOL("__android_log_print", print_log)
    RUDROID_SYMBOL("__android_log_vprint", print_list) RUDROID_SYMBOL("__android_log_assert", assert_log)
    RUDROID_SYMBOL("__android_log_buf_write", buffer_write) RUDROID_SYMBOL("__android_log_buf_print", buffer_print)
};
RUDROID_SYMBOL_PROVIDER(logging, symbols)
