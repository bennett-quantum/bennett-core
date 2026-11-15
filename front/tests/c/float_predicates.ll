; float_predicates.ll: every fcmp predicate, and llvm.minimum and llvm.maximum, on pairs of directed
; values, in LLVM IR because C does not reach all of them: clang's optimiser folds `!(a > b)` and `a < b ||
; a != a` into whichever predicate it prefers, and clang 18 has no builtin for llvm.minimum. result(i) for
; pair i (of 12 x 12 doubles, then floats) is a mask of the 16 predicates in bits 0-15, the bits of the
; minimum from bit 16 and of the maximum above them (a NaN result as the quiet NaN, since which NaN comes
; out is not specified). A C driver prints them natively (floating.rs).
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-linux-gnu"

@doubles = internal global [12 x double] [double 0.0, double -0.0, double 1.0, double -1.0, double 2.0, double 0x0000000000000001, double 0x0010000000000000, double 0x7FEFFFFFFFFFFFFF, double 0x7FF0000000000000, double 0xFFF0000000000000, double 0x7FF8000000000000, double 0xFFF8000000000007]
@floats = internal global [12 x float] [float 0.0, float -0.0, float 1.0, float -1.0, float 2.0, float 0x36A0000000000000, float 0x3810000000000000, float 0x47EFFFFFE0000000, float 0x7FF0000000000000, float 0xFFF0000000000000, float 0x7FF8000000000000, float 0xFFF8000000000000]

define i32 @results() {
  ret i32 288
}

define i128 @result(i32 %i) {
  %wide = zext i32 %i to i64
  %float = icmp uge i64 %wide, 144
  %j = urem i64 %wide, 144
  %a = udiv i64 %j, 12
  %b = urem i64 %j, 12
  br i1 %float, label %f, label %d

d:
  %padouble = getelementptr inbounds [12 x double], ptr @doubles, i64 0, i64 %a
  %pbdouble = getelementptr inbounds [12 x double], ptr @doubles, i64 0, i64 %b
  %xdouble = load volatile double, ptr %padouble
  %ydouble = load volatile double, ptr %pbdouble
  %cdouble0 = fcmp false double %xdouble, %ydouble
  %zdouble0 = zext i1 %cdouble0 to i64
  %sdouble0 = shl i64 %zdouble0, 0
  %mdouble0 = or i64 0, %sdouble0
  %cdouble1 = fcmp oeq double %xdouble, %ydouble
  %zdouble1 = zext i1 %cdouble1 to i64
  %sdouble1 = shl i64 %zdouble1, 1
  %mdouble1 = or i64 %mdouble0, %sdouble1
  %cdouble2 = fcmp ogt double %xdouble, %ydouble
  %zdouble2 = zext i1 %cdouble2 to i64
  %sdouble2 = shl i64 %zdouble2, 2
  %mdouble2 = or i64 %mdouble1, %sdouble2
  %cdouble3 = fcmp oge double %xdouble, %ydouble
  %zdouble3 = zext i1 %cdouble3 to i64
  %sdouble3 = shl i64 %zdouble3, 3
  %mdouble3 = or i64 %mdouble2, %sdouble3
  %cdouble4 = fcmp olt double %xdouble, %ydouble
  %zdouble4 = zext i1 %cdouble4 to i64
  %sdouble4 = shl i64 %zdouble4, 4
  %mdouble4 = or i64 %mdouble3, %sdouble4
  %cdouble5 = fcmp ole double %xdouble, %ydouble
  %zdouble5 = zext i1 %cdouble5 to i64
  %sdouble5 = shl i64 %zdouble5, 5
  %mdouble5 = or i64 %mdouble4, %sdouble5
  %cdouble6 = fcmp one double %xdouble, %ydouble
  %zdouble6 = zext i1 %cdouble6 to i64
  %sdouble6 = shl i64 %zdouble6, 6
  %mdouble6 = or i64 %mdouble5, %sdouble6
  %cdouble7 = fcmp ord double %xdouble, %ydouble
  %zdouble7 = zext i1 %cdouble7 to i64
  %sdouble7 = shl i64 %zdouble7, 7
  %mdouble7 = or i64 %mdouble6, %sdouble7
  %cdouble8 = fcmp ueq double %xdouble, %ydouble
  %zdouble8 = zext i1 %cdouble8 to i64
  %sdouble8 = shl i64 %zdouble8, 8
  %mdouble8 = or i64 %mdouble7, %sdouble8
  %cdouble9 = fcmp ugt double %xdouble, %ydouble
  %zdouble9 = zext i1 %cdouble9 to i64
  %sdouble9 = shl i64 %zdouble9, 9
  %mdouble9 = or i64 %mdouble8, %sdouble9
  %cdouble10 = fcmp uge double %xdouble, %ydouble
  %zdouble10 = zext i1 %cdouble10 to i64
  %sdouble10 = shl i64 %zdouble10, 10
  %mdouble10 = or i64 %mdouble9, %sdouble10
  %cdouble11 = fcmp ult double %xdouble, %ydouble
  %zdouble11 = zext i1 %cdouble11 to i64
  %sdouble11 = shl i64 %zdouble11, 11
  %mdouble11 = or i64 %mdouble10, %sdouble11
  %cdouble12 = fcmp ule double %xdouble, %ydouble
  %zdouble12 = zext i1 %cdouble12 to i64
  %sdouble12 = shl i64 %zdouble12, 12
  %mdouble12 = or i64 %mdouble11, %sdouble12
  %cdouble13 = fcmp une double %xdouble, %ydouble
  %zdouble13 = zext i1 %cdouble13 to i64
  %sdouble13 = shl i64 %zdouble13, 13
  %mdouble13 = or i64 %mdouble12, %sdouble13
  %cdouble14 = fcmp uno double %xdouble, %ydouble
  %zdouble14 = zext i1 %cdouble14 to i64
  %sdouble14 = shl i64 %zdouble14, 14
  %mdouble14 = or i64 %mdouble13, %sdouble14
  %cdouble15 = fcmp true double %xdouble, %ydouble
  %zdouble15 = zext i1 %cdouble15 to i64
  %sdouble15 = shl i64 %zdouble15, 15
  %mdouble15 = or i64 %mdouble14, %sdouble15
  %rdouble0 = call double @llvm.minimum.f64(double %xdouble, double %ydouble)
  %ndouble0 = fcmp uno double %rdouble0, %rdouble0
  %qdouble0 = select i1 %ndouble0, double 0x7FF8000000000000, double %rdouble0
  %idouble0 = bitcast double %qdouble0 to i64
  %wdouble0 = zext i64 %idouble0 to i128
  %rdouble1 = call double @llvm.maximum.f64(double %xdouble, double %ydouble)
  %ndouble1 = fcmp uno double %rdouble1, %rdouble1
  %qdouble1 = select i1 %ndouble1, double 0x7FF8000000000000, double %rdouble1
  %idouble1 = bitcast double %qdouble1 to i64
  %wdouble1 = zext i64 %idouble1 to i128
  %alldouble = zext i64 %mdouble15 to i128
  %lodouble = shl i128 %wdouble0, 16
  %hidouble = shl i128 %wdouble1, 80
  %tdouble = or i128 %alldouble, %lodouble
  %udouble = or i128 %tdouble, %hidouble
  ret i128 %udouble

f:
  %pafloat = getelementptr inbounds [12 x float], ptr @floats, i64 0, i64 %a
  %pbfloat = getelementptr inbounds [12 x float], ptr @floats, i64 0, i64 %b
  %xfloat = load volatile float, ptr %pafloat
  %yfloat = load volatile float, ptr %pbfloat
  %cfloat0 = fcmp false float %xfloat, %yfloat
  %zfloat0 = zext i1 %cfloat0 to i64
  %sfloat0 = shl i64 %zfloat0, 0
  %mfloat0 = or i64 0, %sfloat0
  %cfloat1 = fcmp oeq float %xfloat, %yfloat
  %zfloat1 = zext i1 %cfloat1 to i64
  %sfloat1 = shl i64 %zfloat1, 1
  %mfloat1 = or i64 %mfloat0, %sfloat1
  %cfloat2 = fcmp ogt float %xfloat, %yfloat
  %zfloat2 = zext i1 %cfloat2 to i64
  %sfloat2 = shl i64 %zfloat2, 2
  %mfloat2 = or i64 %mfloat1, %sfloat2
  %cfloat3 = fcmp oge float %xfloat, %yfloat
  %zfloat3 = zext i1 %cfloat3 to i64
  %sfloat3 = shl i64 %zfloat3, 3
  %mfloat3 = or i64 %mfloat2, %sfloat3
  %cfloat4 = fcmp olt float %xfloat, %yfloat
  %zfloat4 = zext i1 %cfloat4 to i64
  %sfloat4 = shl i64 %zfloat4, 4
  %mfloat4 = or i64 %mfloat3, %sfloat4
  %cfloat5 = fcmp ole float %xfloat, %yfloat
  %zfloat5 = zext i1 %cfloat5 to i64
  %sfloat5 = shl i64 %zfloat5, 5
  %mfloat5 = or i64 %mfloat4, %sfloat5
  %cfloat6 = fcmp one float %xfloat, %yfloat
  %zfloat6 = zext i1 %cfloat6 to i64
  %sfloat6 = shl i64 %zfloat6, 6
  %mfloat6 = or i64 %mfloat5, %sfloat6
  %cfloat7 = fcmp ord float %xfloat, %yfloat
  %zfloat7 = zext i1 %cfloat7 to i64
  %sfloat7 = shl i64 %zfloat7, 7
  %mfloat7 = or i64 %mfloat6, %sfloat7
  %cfloat8 = fcmp ueq float %xfloat, %yfloat
  %zfloat8 = zext i1 %cfloat8 to i64
  %sfloat8 = shl i64 %zfloat8, 8
  %mfloat8 = or i64 %mfloat7, %sfloat8
  %cfloat9 = fcmp ugt float %xfloat, %yfloat
  %zfloat9 = zext i1 %cfloat9 to i64
  %sfloat9 = shl i64 %zfloat9, 9
  %mfloat9 = or i64 %mfloat8, %sfloat9
  %cfloat10 = fcmp uge float %xfloat, %yfloat
  %zfloat10 = zext i1 %cfloat10 to i64
  %sfloat10 = shl i64 %zfloat10, 10
  %mfloat10 = or i64 %mfloat9, %sfloat10
  %cfloat11 = fcmp ult float %xfloat, %yfloat
  %zfloat11 = zext i1 %cfloat11 to i64
  %sfloat11 = shl i64 %zfloat11, 11
  %mfloat11 = or i64 %mfloat10, %sfloat11
  %cfloat12 = fcmp ule float %xfloat, %yfloat
  %zfloat12 = zext i1 %cfloat12 to i64
  %sfloat12 = shl i64 %zfloat12, 12
  %mfloat12 = or i64 %mfloat11, %sfloat12
  %cfloat13 = fcmp une float %xfloat, %yfloat
  %zfloat13 = zext i1 %cfloat13 to i64
  %sfloat13 = shl i64 %zfloat13, 13
  %mfloat13 = or i64 %mfloat12, %sfloat13
  %cfloat14 = fcmp uno float %xfloat, %yfloat
  %zfloat14 = zext i1 %cfloat14 to i64
  %sfloat14 = shl i64 %zfloat14, 14
  %mfloat14 = or i64 %mfloat13, %sfloat14
  %cfloat15 = fcmp true float %xfloat, %yfloat
  %zfloat15 = zext i1 %cfloat15 to i64
  %sfloat15 = shl i64 %zfloat15, 15
  %mfloat15 = or i64 %mfloat14, %sfloat15
  %rfloat0 = call float @llvm.minimum.f32(float %xfloat, float %yfloat)
  %nfloat0 = fcmp uno float %rfloat0, %rfloat0
  %qfloat0 = select i1 %nfloat0, float 0x7FF8000000000000, float %rfloat0
  %ifloat0 = bitcast float %qfloat0 to i32
  %wfloat0 = zext i32 %ifloat0 to i128
  %rfloat1 = call float @llvm.maximum.f32(float %xfloat, float %yfloat)
  %nfloat1 = fcmp uno float %rfloat1, %rfloat1
  %qfloat1 = select i1 %nfloat1, float 0x7FF8000000000000, float %rfloat1
  %ifloat1 = bitcast float %qfloat1 to i32
  %wfloat1 = zext i32 %ifloat1 to i128
  %allfloat = zext i64 %mfloat15 to i128
  %lofloat = shl i128 %wfloat0, 16
  %hifloat = shl i128 %wfloat1, 48
  %tfloat = or i128 %allfloat, %lofloat
  %ufloat = or i128 %tfloat, %hifloat
  ret i128 %ufloat
}

declare double @llvm.minimum.f64(double, double)
declare double @llvm.maximum.f64(double, double)
declare float @llvm.minimum.f32(float, float)
declare float @llvm.maximum.f32(float, float)
