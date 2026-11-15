; Exact LLVM aggregate shapes, independent of the platform C ABI's register classification.
; result() has an ordinary C i128 result; its C driver is also the native oracle. Some layouts have
; padding beyond 128 bits but exactly 128 bits of scalar fields. One poison field is deliberately unread.
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-linux-gnu"
@word = global i64 81985529216486895
@ptr0 = global ptr @make0
@ptr1 = global ptr @make1
@ptr2 = global ptr @make2
@ptr3 = global ptr @make3
@ptr4 = global ptr @make4
@ptr5 = global ptr @make5
@ptr6 = global ptr @make6
@ptr7 = global ptr @make7
@ptr8 = global ptr @make8
@ptr9 = global ptr @make9
@ptr10 = global ptr @make10
@ptr11 = global ptr @make11

define { i1, i7, i24, i32, i64 } @make0(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %int0 = trunc i128 %shift0 to i1
  %r0 = insertvalue { i1, i7, i24, i32, i64 } poison, i1 %int0, 0
  %shift1 = lshr i128 %wide, 7
  %int1 = trunc i128 %shift1 to i7
  %r1 = insertvalue { i1, i7, i24, i32, i64 } %r0, i7 %int1, 1
  %shift2 = lshr i128 %wide, 14
  %int2 = trunc i128 %shift2 to i24
  %r2 = insertvalue { i1, i7, i24, i32, i64 } %r1, i24 %int2, 2
  %shift3 = lshr i128 %wide, 21
  %int3 = trunc i128 %shift3 to i32
  %r3 = insertvalue { i1, i7, i24, i32, i64 } %r2, i32 %int3, 3
  %shift4 = lshr i128 %wide, 28
  %int4 = trunc i128 %shift4 to i64
  %r4 = insertvalue { i1, i7, i24, i32, i64 } %r3, i64 %int4, 4
  ret { i1, i7, i24, i32, i64 } %r4
}

define i128 @use0(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr0
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make0, ptr %p
  %r = call { i1, i7, i24, i32, i64 } %f(i64 %x)
  %v0 = extractvalue { i1, i7, i24, i32, i64 } %r, 0
  %w0 = zext i1 %v0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  %v1 = extractvalue { i1, i7, i24, i32, i64 } %r, 1
  %w1 = zext i7 %v1 to i128
  %l1 = shl i128 %s0, 17
  %h1 = lshr i128 %s0, 111
  %rot1 = or i128 %l1, %h1
  %s1 = xor i128 %rot1, %w1
  %v2 = extractvalue { i1, i7, i24, i32, i64 } %r, 2
  %w2 = zext i24 %v2 to i128
  %l2 = shl i128 %s1, 17
  %h2 = lshr i128 %s1, 111
  %rot2 = or i128 %l2, %h2
  %s2 = xor i128 %rot2, %w2
  %v3 = extractvalue { i1, i7, i24, i32, i64 } %r, 3
  %w3 = zext i32 %v3 to i128
  %l3 = shl i128 %s2, 17
  %h3 = lshr i128 %s2, 111
  %rot3 = or i128 %l3, %h3
  %s3 = xor i128 %rot3, %w3
  %v4 = extractvalue { i1, i7, i24, i32, i64 } %r, 4
  %w4 = zext i64 %v4 to i128
  %l4 = shl i128 %s3, 17
  %h4 = lshr i128 %s3, 111
  %rot4 = or i128 %l4, %h4
  %s4 = xor i128 %rot4, %w4
  ret i128 %s4
}

define <{ i1, i64 }> @make1(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %int0 = trunc i128 %shift0 to i1
  %r0 = insertvalue <{ i1, i64 }> poison, i1 %int0, 0
  %shift1 = lshr i128 %wide, 7
  %int1 = trunc i128 %shift1 to i64
  %r1 = insertvalue <{ i1, i64 }> %r0, i64 %int1, 1
  ret <{ i1, i64 }> %r1
}

define i128 @use1(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr1
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make1, ptr %p
  %r = call <{ i1, i64 }> %f(i64 %x)
  %v0 = extractvalue <{ i1, i64 }> %r, 0
  %w0 = zext i1 %v0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  %v1 = extractvalue <{ i1, i64 }> %r, 1
  %w1 = zext i64 %v1 to i128
  %l1 = shl i128 %s0, 17
  %h1 = lshr i128 %s0, 111
  %rot1 = or i128 %l1, %h1
  %s1 = xor i128 %rot1, %w1
  ret i128 %s1
}

define { [2 x i8], { i16, [2 x i32] } } @make2(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %int0 = trunc i128 %shift0 to i8
  %r0 = insertvalue { [2 x i8], { i16, [2 x i32] } } poison, i8 %int0, 0, 0
  %shift1 = lshr i128 %wide, 7
  %int1 = trunc i128 %shift1 to i8
  %r1 = insertvalue { [2 x i8], { i16, [2 x i32] } } %r0, i8 %int1, 0, 1
  %shift2 = lshr i128 %wide, 14
  %int2 = trunc i128 %shift2 to i16
  %r2 = insertvalue { [2 x i8], { i16, [2 x i32] } } %r1, i16 %int2, 1, 0
  %shift3 = lshr i128 %wide, 21
  %int3 = trunc i128 %shift3 to i32
  %r3 = insertvalue { [2 x i8], { i16, [2 x i32] } } %r2, i32 %int3, 1, 1, 0
  %shift4 = lshr i128 %wide, 28
  %int4 = trunc i128 %shift4 to i32
  %r4 = insertvalue { [2 x i8], { i16, [2 x i32] } } %r3, i32 %int4, 1, 1, 1
  ret { [2 x i8], { i16, [2 x i32] } } %r4
}

define i128 @use2(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr2
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make2, ptr %p
  %r = call { [2 x i8], { i16, [2 x i32] } } %f(i64 %x)
  %v0 = extractvalue { [2 x i8], { i16, [2 x i32] } } %r, 0, 0
  %w0 = zext i8 %v0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  %v1 = extractvalue { [2 x i8], { i16, [2 x i32] } } %r, 0, 1
  %w1 = zext i8 %v1 to i128
  %l1 = shl i128 %s0, 17
  %h1 = lshr i128 %s0, 111
  %rot1 = or i128 %l1, %h1
  %s1 = xor i128 %rot1, %w1
  %v2 = extractvalue { [2 x i8], { i16, [2 x i32] } } %r, 1, 0
  %w2 = zext i16 %v2 to i128
  %l2 = shl i128 %s1, 17
  %h2 = lshr i128 %s1, 111
  %rot2 = or i128 %l2, %h2
  %s2 = xor i128 %rot2, %w2
  %v3 = extractvalue { [2 x i8], { i16, [2 x i32] } } %r, 1, 1, 0
  %w3 = zext i32 %v3 to i128
  %l3 = shl i128 %s2, 17
  %h3 = lshr i128 %s2, 111
  %rot3 = or i128 %l3, %h3
  %s3 = xor i128 %rot3, %w3
  %v4 = extractvalue { [2 x i8], { i16, [2 x i32] } } %r, 1, 1, 1
  %w4 = zext i32 %v4 to i128
  %l4 = shl i128 %s3, 17
  %h4 = lshr i128 %s3, 111
  %rot4 = or i128 %l4, %h4
  %s4 = xor i128 %rot4, %w4
  ret i128 %s4
}

define { i64, i64, i1 } @make3(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %int0 = trunc i128 %shift0 to i64
  %r0 = insertvalue { i64, i64, i1 } poison, i64 %int0, 0
  %shift1 = lshr i128 %wide, 7
  %int1 = trunc i128 %shift1 to i64
  %r1 = insertvalue { i64, i64, i1 } %r0, i64 %int1, 1
  %shift2 = lshr i128 %wide, 14
  %int2 = trunc i128 %shift2 to i1
  %r2 = insertvalue { i64, i64, i1 } %r1, i1 %int2, 2
  ret { i64, i64, i1 } %r2
}

define i128 @use3(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr3
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make3, ptr %p
  %r = call { i64, i64, i1 } %f(i64 %x)
  %v0 = extractvalue { i64, i64, i1 } %r, 0
  %w0 = zext i64 %v0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  %v1 = extractvalue { i64, i64, i1 } %r, 1
  %w1 = zext i64 %v1 to i128
  %l1 = shl i128 %s0, 17
  %h1 = lshr i128 %s0, 111
  %rot1 = or i128 %l1, %h1
  %s1 = xor i128 %rot1, %w1
  %v2 = extractvalue { i64, i64, i1 } %r, 2
  %w2 = zext i1 %v2 to i128
  %l2 = shl i128 %s1, 17
  %h2 = lshr i128 %s1, 111
  %rot2 = or i128 %l2, %h2
  %s2 = xor i128 %rot2, %w2
  ret i128 %s2
}

define { i128 } @make4(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %r0 = insertvalue { i128 } poison, i128 %shift0, 0
  ret { i128 } %r0
}

define i128 @use4(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr4
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make4, ptr %p
  %r = call { i128 } %f(i64 %x)
  %v0 = extractvalue { i128 } %r, 0
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %v0
  ret i128 %s0
}

define { i64, ptr } @make5(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %int0 = trunc i128 %shift0 to i64
  %r0 = insertvalue { i64, ptr } poison, i64 %int0, 0
  %r1 = insertvalue { i64, ptr } %r0, ptr @word, 1
  ret { i64, ptr } %r1
}

define i128 @use5(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr5
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make5, ptr %p
  %r = call { i64, ptr } %f(i64 %x)
  %v0 = extractvalue { i64, ptr } %r, 0
  %w0 = zext i64 %v0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  %v1 = extractvalue { i64, ptr } %r, 1
  %n1 = load i64, ptr %v1
  %w1 = zext i64 %n1 to i128
  %l1 = shl i128 %s0, 17
  %h1 = lshr i128 %s0, 111
  %rot1 = or i128 %l1, %h1
  %s1 = xor i128 %rot1, %w1
  ret i128 %s1
}

define { <2 x i64> } @make6(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %v0.0 = insertelement <2 x i64> poison, i64 %x, i32 0
  %v0 = insertelement <2 x i64> %v0.0, i64 12345, i32 1
  %r0 = insertvalue { <2 x i64> } poison, <2 x i64> %v0, 0
  ret { <2 x i64> } %r0
}

define i128 @use6(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr6
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make6, ptr %p
  %r = call { <2 x i64> } %f(i64 %x)
  %v0 = extractvalue { <2 x i64> } %r, 0
  %w0 = bitcast <2 x i64> %v0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  ret i128 %s0
}

define { [0 x i8], i64, {} } @make7(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %int0 = trunc i128 %shift0 to i64
  %r0 = insertvalue { [0 x i8], i64, {} } poison, i64 %int0, 1
  ret { [0 x i8], i64, {} } %r0
}

define i128 @use7(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr7
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make7, ptr %p
  %r = call { [0 x i8], i64, {} } %f(i64 %x)
  %v0 = extractvalue { [0 x i8], i64, {} } %r, 1
  %w0 = zext i64 %v0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  ret i128 %s0
}

define { x86_fp80, float, half } @make8(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %int0 = trunc i128 %shift0 to i80
  %f0 = bitcast i80 %int0 to x86_fp80
  %r0 = insertvalue { x86_fp80, float, half } poison, x86_fp80 %f0, 0
  %shift1 = lshr i128 %wide, 7
  %int1 = trunc i128 %shift1 to i32
  %f1 = bitcast i32 %int1 to float
  %r1 = insertvalue { x86_fp80, float, half } %r0, float %f1, 1
  %shift2 = lshr i128 %wide, 14
  %int2 = trunc i128 %shift2 to i16
  %f2 = bitcast i16 %int2 to half
  %r2 = insertvalue { x86_fp80, float, half } %r1, half %f2, 2
  ret { x86_fp80, float, half } %r2
}

define i128 @use8(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr8
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make8, ptr %p
  %r = call { x86_fp80, float, half } %f(i64 %x)
  %v0 = extractvalue { x86_fp80, float, half } %r, 0
  %n0 = bitcast x86_fp80 %v0 to i80
  %w0 = zext i80 %n0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  %v1 = extractvalue { x86_fp80, float, half } %r, 1
  %n1 = bitcast float %v1 to i32
  %w1 = zext i32 %n1 to i128
  %l1 = shl i128 %s0, 17
  %h1 = lshr i128 %s0, 111
  %rot1 = or i128 %l1, %h1
  %s1 = xor i128 %rot1, %w1
  %v2 = extractvalue { x86_fp80, float, half } %r, 2
  %n2 = bitcast half %v2 to i16
  %w2 = zext i16 %n2 to i128
  %l2 = shl i128 %s1, 17
  %h2 = lshr i128 %s1, 111
  %rot2 = or i128 %l2, %h2
  %s2 = xor i128 %rot2, %w2
  ret i128 %s2
}

define { i64, i64 } @make9(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %int0 = trunc i128 %shift0 to i64
  %r0 = insertvalue { i64, i64 } poison, i64 %int0, 0
  ret { i64, i64 } %r0
}

define i128 @use9(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr9
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make9, ptr %p
  %r = call { i64, i64 } %f(i64 %x)
  %v0 = extractvalue { i64, i64 } %r, 0
  %w0 = zext i64 %v0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  ret i128 %s0
}

define { i64, [0 x i8] } @make10(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %int0 = trunc i128 %shift0 to i64
  %r0 = insertvalue { i64, [0 x i8] } poison, i64 %int0, 0
  ret { i64, [0 x i8] } %r0
}

define i128 @use10(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr10
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make10, ptr %p
  %r = call { i64, [0 x i8] } %f(i64 %x)
  %v0 = extractvalue { i64, [0 x i8] } %r, 0
  %w0 = zext i64 %v0 to i128
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %w0
  ret i128 %s0
}

define { i128, i1 } @make11(i64 %x) noinline {
  %z = zext i64 %x to i128
  %hi = shl i128 %z, 64
  %wide = or i128 %z, %hi
  %shift0 = lshr i128 %wide, 0
  %r0 = insertvalue { i128, i1 } poison, i128 %shift0, 0
  %shift1 = lshr i128 %wide, 7
  %int1 = trunc i128 %shift1 to i1
  %r1 = insertvalue { i128, i1 } %r0, i1 %int1, 1
  ret { i128, i1 } %r1
}

define i128 @use11(i64 %x, i32 %mode) noinline {
  %p = load volatile ptr, ptr @ptr11
  %direct = icmp eq i32 %mode, 0
  %f = select i1 %direct, ptr @make11, ptr %p
  %r = call { i128, i1 } %f(i64 %x)
  %v0 = extractvalue { i128, i1 } %r, 0
  %l0 = shl i128 0, 17
  %h0 = lshr i128 0, 111
  %rot0 = or i128 %l0, %h0
  %s0 = xor i128 %rot0, %v0
  %v1 = extractvalue { i128, i1 } %r, 1
  %w1 = zext i1 %v1 to i128
  %l1 = shl i128 %s0, 17
  %h1 = lshr i128 %s0, 111
  %rot1 = or i128 %l1, %h1
  %s1 = xor i128 %rot1, %w1
  ret i128 %s1
}

define i128 @result(i32 %which, i64 %x, i32 %mode) {
  switch i32 %which, label %bad [
    i32 0, label %c0
    i32 1, label %c1
    i32 2, label %c2
    i32 3, label %c3
    i32 4, label %c4
    i32 5, label %c5
    i32 6, label %c6
    i32 7, label %c7
    i32 8, label %c8
    i32 9, label %c9
    i32 10, label %c10
    i32 11, label %c11
  ]
c0:
  %r0 = call i128 @use0(i64 %x, i32 %mode)
  ret i128 %r0
c1:
  %r1 = call i128 @use1(i64 %x, i32 %mode)
  ret i128 %r1
c2:
  %r2 = call i128 @use2(i64 %x, i32 %mode)
  ret i128 %r2
c3:
  %r3 = call i128 @use3(i64 %x, i32 %mode)
  ret i128 %r3
c4:
  %r4 = call i128 @use4(i64 %x, i32 %mode)
  ret i128 %r4
c5:
  %r5 = call i128 @use5(i64 %x, i32 %mode)
  ret i128 %r5
c6:
  %r6 = call i128 @use6(i64 %x, i32 %mode)
  ret i128 %r6
c7:
  %r7 = call i128 @use7(i64 %x, i32 %mode)
  ret i128 %r7
c8:
  %r8 = call i128 @use8(i64 %x, i32 %mode)
  ret i128 %r8
c9:
  %r9 = call i128 @use9(i64 %x, i32 %mode)
  ret i128 %r9
c10:
  %r10 = call i128 @use10(i64 %x, i32 %mode)
  ret i128 %r10
c11:
  %r11 = call i128 @use11(i64 %x, i32 %mode)
  ret i128 %r11
bad:
  ret i128 0
}
