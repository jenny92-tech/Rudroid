// INPUT:  versioned libm.so float/double/FP128 and shared native errno
// OUTPUT:  rudroid_math_probe numeric, min/max NaN/signed-zero and pointer-output ABI checks
// POS:    verifies actual guest calls into the single native libm implementation
extern double sin(double),cos(double),pow(double,double),modf(double,double *),ldexp(double,int);
extern float sqrtf(float),nexttowardf(float,long double),atan2f(float,float);
extern double fmin(double,double),fmax(double,double);
extern float fminf(float,float),fmaxf(float,float);
extern void sincos(double,double *,double *);
extern int *__errno(void);
int rudroid_math_probe(void) {
    double s,c,integer;
    sincos(0,&s,&c);
    if (s!=0 || c!=1 || sin(0)!=0 || cos(0)!=1 || pow(2,3)!=8) return 1;
    if (sqrtf(9)!=3 || ldexp(1,8)!=256 || modf(1.25,&integer)!=0.25 || integer!=1) return 2;
    if (nexttowardf(1,1.00000000000000000000000000000001L)!=0x1.000002p0f) return 3;
    if (atan2f(0,-1)<3.14f || atan2f(0,-1)>3.15f) return 4;
    *__errno()=0;
    volatile double negative=-1,half=0.5;
    double invalid=pow(negative,half);
    if (invalid==invalid || *__errno()!=33) return 5;
    union { unsigned long bits; double value; } nan={.bits=0x7ff8000000000001UL},dn={.bits=0x8000000000000000UL},d;
    union { unsigned bits; float value; } fnan={.bits=0x7fc00001u},fn={.bits=0x80000000u},f;
    if (fmin(-2.5,7.25)!=-2.5 || fmax(7.25,-2.5)!=7.25 || fminf(-2.5f,7.25f)!=-2.5f || fmaxf(7.25f,-2.5f)!=7.25f) return 6;
    if (fmin(nan.value,2)!=2 || fmin(2,nan.value)!=2 || fmax(nan.value,2)!=2 || fmax(2,nan.value)!=2) return 7;
    if (fminf(fnan.value,2)!=2 || fminf(2,fnan.value)!=2 || fmaxf(fnan.value,2)!=2 || fmaxf(2,fnan.value)!=2) return 8;
    d.value=fmin(nan.value,nan.value); if (d.value==d.value) return 9;
    d.value=fmax(nan.value,nan.value); if (d.value==d.value) return 10;
    f.value=fminf(fnan.value,fnan.value); if (f.value==f.value) return 11;
    f.value=fmaxf(fnan.value,fnan.value); if (f.value==f.value) return 12;
    for (int reverse=0;reverse<2;++reverse) {
        double x=reverse ? dn.value : 0.0,y=reverse ? 0.0 : dn.value;
        d.value=fmin(x,y); if (d.bits!=dn.bits) return 13;
        d.value=fmax(x,y); if (d.bits!=0) return 14;
        float xf=reverse ? fn.value : 0.0f,yf=reverse ? 0.0f : fn.value;
        f.value=fminf(xf,yf); if (f.bits!=fn.bits) return 15;
        f.value=fmaxf(xf,yf); if (f.bits!=0) return 16;
    }
    union { unsigned long bits; double value; } infinity={.bits=0x7ff0000000000000UL};
    union { unsigned bits; float value; } finfinity={.bits=0x7f800000u};
    if (fmin(infinity.value,-infinity.value)!=-infinity.value || fmax(-infinity.value,infinity.value)!=infinity.value ||
        fminf(finfinity.value,-finfinity.value)!=-finfinity.value || fmaxf(-finfinity.value,finfinity.value)!=finfinity.value) return 17;
    return 0;
}
