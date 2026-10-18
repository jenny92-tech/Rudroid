// INPUT:  versioned libm.so float/double/FP128 and shared native errno
// OUTPUT:  rudroid_math_probe numeric and pointer-output ABI checks
// POS:    verifies actual guest calls into the single native libm implementation
extern double sin(double),cos(double),pow(double,double),modf(double,double *),ldexp(double,int);
extern float sqrtf(float),nexttowardf(float,long double),atan2f(float,float);
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
    return 0;
}
