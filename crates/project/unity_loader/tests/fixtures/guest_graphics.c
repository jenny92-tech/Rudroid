// INPUT:  EGL/GLES headers and the guest loader's same-service ANativeWindow imports
// OUTPUT:  rudroid_graphics_begin/frame/end probes, or an empty support SO
// POS:    Executes guest graphics through ELF relocations and dynamic proc lookup
#ifdef RUDROID_PLACEHOLDER
int rudroid_empty(void) { return 0; }
#else
#include <EGL/egl.h>
#include <GLES3/gl3.h>
#include <stdint.h>
typedef struct ANativeWindow ANativeWindow;
extern ANativeWindow *ANativeWindow_fromSurface(void *env,void *surface);
extern void ANativeWindow_acquire(ANativeWindow *window);
extern void ANativeWindow_release(ANativeWindow *window);
extern int32_t ANativeWindow_getWidth(ANativeWindow *window);
extern int32_t ANativeWindow_getHeight(ANativeWindow *window);
extern int32_t ANativeWindow_setBuffersGeometry(ANativeWindow *window,int32_t width,int32_t height,int32_t format);

static EGLDisplay display;
static EGLSurface surface;
static EGLContext context;
static GLuint shaders[2], program, buffer, texture;
static ANativeWindow *window;

int rudroid_graphics_begin(void) {
    display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (display == EGL_NO_DISPLAY || !eglInitialize(display, 0, 0)) return 10;
    EGLConfig config = 0;
    EGLint count = 0;
    if (!eglChooseConfig(display, 0, &config, 1, &count) || count != 1) return 11;
    window=ANativeWindow_fromSurface(0,(void *)1);
    if (!window || ANativeWindow_fromSurface(0,0)) return 16;
    ANativeWindow_acquire(window); ANativeWindow_release(window);
    if (ANativeWindow_setBuffersGeometry(window,0,0,0) ||
        ANativeWindow_setBuffersGeometry(window,64,64,0) ||
        ANativeWindow_setBuffersGeometry(window,32,32,0)!=-95 ||
        ANativeWindow_setBuffersGeometry(window,0,0,1)!=-95 ||
        ANativeWindow_setBuffersGeometry(window,0,64,0)!=-22) return 17;
    // The owning guest token maps to the same surface, never a Linux window pointer.
    surface = eglCreateWindowSurface(display, config, (EGLNativeWindowType)window, 0);
    const EGLint attrs[] = { EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE };
    context = eglCreateContext(display, config, EGL_NO_CONTEXT, attrs);
    if (!surface || !context || !eglMakeCurrent(display, surface, surface, context)) return 12;
    if (eglGetCurrentContext() != context || eglGetCurrentSurface(EGL_DRAW) != surface) return 13;
    if (eglGetProcAddress("glClearColor") != (void (*)(void))glClearColor) return 14;
    if (eglGetProcAddress("eglSwapBuffers") != (void (*)(void))eglSwapBuffers) return 15;
    const char *sources[] = {
        "#version 300 es\nlayout(location=0) in vec2 position; void main(){gl_Position=vec4(position,0.0,1.0);}",
        "#version 300 es\nprecision mediump float; out vec4 color; void main(){color=vec4(0.25,0.5,0.75,1.0);}"
    };
    const GLenum types[] = { GL_VERTEX_SHADER, GL_FRAGMENT_SHADER };
    for (int i = 0; i < 2; ++i) {
        shaders[i] = glCreateShader(types[i]);
        glShaderSource(shaders[i], 1, &sources[i], 0);
        glCompileShader(shaders[i]);
        GLint status = 0;
        glGetShaderiv(shaders[i], GL_COMPILE_STATUS, &status);
        if (!status) return 20 + i;
    }
    program = glCreateProgram();
    glAttachShader(program, shaders[0]); glAttachShader(program, shaders[1]);
    glLinkProgram(program);
    GLint status = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &status);
    if (!status) return 22;
    const float vertices[] = { -1.f,-1.f, 1.f,-1.f, 0.f,1.f };
    glGenBuffers(1, &buffer);
    glBindBuffer(GL_ARRAY_BUFFER, buffer);
    glBufferData(GL_ARRAY_BUFFER, sizeof(vertices), vertices, GL_STATIC_DRAW);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, 0);
    glEnableVertexAttribArray(0);
    glGenTextures(1, &texture);
    glBindTexture(GL_TEXTURE_2D, texture);
    const unsigned char pixel[] = { 255, 0, 0, 255 };
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, 1, 1, 0, GL_RGBA, GL_UNSIGNED_BYTE, pixel);
    if (!glIsTexture(texture)) return 23;
    return glGetError() == GL_NO_ERROR ? 0 : 24;
}

int rudroid_graphics_frame(void) {
    EGLint width = 0, height = 0;
    if (!eglQuerySurface(display, surface, EGL_WIDTH, &width) ||
        !eglQuerySurface(display, surface, EGL_HEIGHT, &height)) return 30;
    if (ANativeWindow_getWidth(window)!=width || ANativeWindow_getHeight(window)!=height) return 34;
    glViewport(0, 0, width, height);
    glClearColor(0.f, 0.f, 0.f, 1.f);
    glClear(GL_COLOR_BUFFER_BIT);
    glUseProgram(program);
    glDrawArrays(GL_TRIANGLES, 0, 3);
    unsigned char pixel[4];
    glReadPixels(width/2, height/2, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, pixel);
    if (pixel[0] < 63 || pixel[0] > 65 || pixel[1] < 127 || pixel[1] > 129 || pixel[2] < 190 || pixel[2] > 192) return 31;
    if (glGetError() != GL_NO_ERROR) return 32;
    return eglSwapBuffers(display, surface) ? 0 : 33;
}

int rudroid_graphics_end(void) {
    glUseProgram(0);
    glDisableVertexAttribArray(0);
    glBindBuffer(GL_ARRAY_BUFFER, 0);
    glBindTexture(GL_TEXTURE_2D, 0);
    glDeleteBuffers(1, &buffer);
    glDeleteTextures(1, &texture);
    glDeleteProgram(program);
    glDeleteShader(shaders[0]); glDeleteShader(shaders[1]);
    if (glIsBuffer(buffer) || glIsTexture(texture) || glIsProgram(program) || glIsShader(shaders[0]) || glIsShader(shaders[1])) return 40;
    if (glGetError() != GL_NO_ERROR) return 41;
    if (!eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT)) return 42;
    if (!eglDestroyContext(display, context) || !eglDestroySurface(display, surface)) return 43;
    ANativeWindow_release(window); window=0;
    return eglReleaseThread() ? 0 : 44;
}
#endif
