; float.asm — IEEE 754 single-precision soft-float for Intel 8080
;
; Format: [sign:1][exponent:8][mantissa:23] — little-endian in memory
;   byte+0 = bits 7..0   (mantissa low)
;   byte+1 = bits 15..8  (mantissa mid)
;   byte+2 = bits 23..16 (mantissa high[6:0] | exponent[0])
;   byte+3 = bits 31..24 (exponent[7:1] | sign)
;
; Calling convention:
;   Inputs:  __op1 (4 bytes), __op2 (4 bytes)
;   Output:  __op1 (4 bytes), HL = low 16 bits of __op1
;   Comparisons return result in HL (0 or 1).
;
; Internal scratch:
;   __fa_s1, __fa_e1, __fa_m1 (4 bytes) — unpacked float 1
;   __fa_s2, __fa_e2, __fa_m2 (4 bytes) — unpacked float 2
;   __fa_tmp (8 bytes) — scratch for multiply/divide

; =====================================================================
; __funpack1 — unpack __op1 into sign/exp/mantissa
;   __fa_s1 = sign (0 or 1)
;   __fa_e1 = biased exponent (8 bits)
;   __fa_m1 = mantissa with implicit bit (24 bits in 4 bytes, little-endian)
; =====================================================================
__funpack1:
	; Extract sign from byte 3, bit 7
	LDA __op1+3
	RLC
	ANI 1
	STA __fa_s1
	; Extract exponent: byte3[6:0]<<1 | byte2[7]
	LDA __op1+3
	ANI 07Fh
	RLC                     ; shift left 1
	MOV B,A                 ; B = byte3[6:0] << 1
	LDA __op1+2
	RLC                     ; carry = byte2[7]
	MOV A,B
	ADC A                   ; no, wrong — we need to OR in carry
	; Redo: extract exponent properly
	; exp = ((byte3 & 0x7F) << 1) | (byte2 >> 7)
	LDA __op1+3
	ANI 07Fh
	ADD A                   ; *2 = shift left 1
	MOV B,A
	LDA __op1+2
	RLC                     ; bit 7 -> carry
	MOV A,B
	ACI 0                   ; add carry
	STA __fa_e1
	; Extract mantissa (23 bits) and add implicit 1 if exp != 0
	LDA __op1
	STA __fa_m1             ; mantissa byte 0
	LDA __op1+1
	STA __fa_m1+1           ; mantissa byte 1
	LDA __op1+2
	ANI 07Fh                ; clear exponent bit, keep mantissa[22:16]
	STA __fa_m1+2           ; mantissa byte 2 (bits 22..16)
	XRA A
	STA __fa_m1+3           ; clear byte 3
	; Set implicit bit 23 if exponent is nonzero (normalized)
	LDA __fa_e1
	ORA A
	JZ __funpack1_done
	LDA __fa_m1+2
	ORI 080h                ; set bit 7 of byte 2 = bit 23 of mantissa
	STA __fa_m1+2
__funpack1_done:
	RET

; =====================================================================
; __funpack2 — unpack __op2 into s2/e2/m2
; =====================================================================
__funpack2:
	LDA __op2+3
	RLC
	ANI 1
	STA __fa_s2
	LDA __op2+3
	ANI 07Fh
	ADD A
	MOV B,A
	LDA __op2+2
	RLC
	MOV A,B
	ACI 0
	STA __fa_e2
	LDA __op2
	STA __fa_m2
	LDA __op2+1
	STA __fa_m2+1
	LDA __op2+2
	ANI 07Fh
	STA __fa_m2+2
	XRA A
	STA __fa_m2+3
	LDA __fa_e2
	ORA A
	JZ __funpack2_done
	LDA __fa_m2+2
	ORI 080h
	STA __fa_m2+2
__funpack2_done:
	RET

; =====================================================================
; __fpack — pack s1/e1/m1 back into __op1 (IEEE 754)
;   Assumes m1 is normalized with bit 23 set (or zero).
; =====================================================================
__fpack:
	; Check for zero mantissa -> return +0 or -0
	LDA __fa_m1
	MOV B,A
	LDA __fa_m1+1
	ORA B
	MOV B,A
	LDA __fa_m1+2
	ORA B
	JNZ __fpack_nonzero
	; Zero result
	XRA A
	STA __op1
	STA __op1+1
	STA __op1+2
	STA __op1+3
	LHLD __op1
	RET
__fpack_nonzero:
	; byte2 = (mantissa byte2 & 0x7F) | ((exp & 1) << 7)
	LDA __fa_e1
	ANI 1
	RRC                     ; bit 0 -> bit 7
	MOV B,A                 ; B = (exp & 1) << 7
	LDA __fa_m1+2
	ANI 07Fh                ; clear implicit bit
	ORA B
	STA __op1+2
	; byte3 = (exp >> 1) | (sign << 7)
	LDA __fa_e1
	ORA A
	RAR                     ; exp >> 1, bit 7 = 0
	MOV B,A
	LDA __fa_s1
	ORA A
	JZ __fpack_nosign
	MOV A,B
	ORI 080h                ; set sign bit
	MOV B,A
__fpack_nosign:
	MOV A,B
	STA __op1+3
	; byte0, byte1 = mantissa low bytes
	LDA __fa_m1
	STA __op1
	LDA __fa_m1+1
	STA __op1+1
	LHLD __op1
	RET

; =====================================================================
; __fnorm — normalize m1/e1: shift mantissa left until bit 23 is set,
;           decrementing exponent. Also handles overflow (bit 24 set).
; =====================================================================
__fnorm:
	; First check if the mantissa is zero
	LDA __fa_m1
	MOV B,A
	LDA __fa_m1+1
	ORA B
	MOV B,A
	LDA __fa_m1+2
	ORA B
	MOV B,A
	LDA __fa_m1+3
	ORA B
	RZ                      ; zero mantissa, nothing to normalize
	; Check for overflow: if bit 24 (byte3 bit 0) is set, shift right
__fnorm_overflow:
	LDA __fa_m1+3
	ANI 1
	JZ __fnorm_up
	; Shift mantissa right by 1 and increment exponent
	LDA __fa_m1+3
	ORA A
	RAR
	STA __fa_m1+3
	LDA __fa_m1+2
	RAR
	STA __fa_m1+2
	LDA __fa_m1+1
	RAR
	STA __fa_m1+1
	LDA __fa_m1
	RAR
	STA __fa_m1
	LDA __fa_e1
	INR A
	STA __fa_e1
	JMP __fnorm_overflow    ; check again in case of multi-bit overflow
__fnorm_up:
	; Shift mantissa left until bit 23 (byte2 bit 7) is set
	LDA __fa_m1+2
	ANI 080h
	RNZ                     ; already normalized
	; Check exponent underflow
	LDA __fa_e1
	ORA A
	RZ                      ; exponent = 0, denormalized, stop
	; Shift m1 left by 1
	LDA __fa_m1
	ADD A
	STA __fa_m1
	LDA __fa_m1+1
	ADC A
	STA __fa_m1+1
	LDA __fa_m1+2
	ADC A
	STA __fa_m1+2
	LDA __fa_m1+3
	ADC A
	STA __fa_m1+3
	; Decrement exponent
	LDA __fa_e1
	DCR A
	STA __fa_e1
	JMP __fnorm_up

; =====================================================================
; __fadd — float add: __op1 = __op1 + __op2
; =====================================================================
__fadd:
	CALL __funpack1
	CALL __funpack2
	; Check for zero operands
	LDA __fa_e1
	MOV B,A
	LDA __fa_m1
	ORA B
	LDA __fa_m1+1
	ORA B
	LDA __fa_m1+2
	ORA B
	JNZ __fadd_op1_nz
	; op1 is zero, result = op2: copy op2 to op1
	LDA __op2
	STA __op1
	LDA __op2+1
	STA __op1+1
	LDA __op2+2
	STA __op1+2
	LDA __op2+3
	STA __op1+3
	LHLD __op1
	RET
__fadd_op1_nz:
	LDA __fa_e2
	MOV B,A
	LDA __fa_m2
	ORA B
	LDA __fa_m2+1
	ORA B
	LDA __fa_m2+2
	ORA B
	JNZ __fadd_both_nz
	; op2 is zero, result = op1 (already there)
	LHLD __op1
	RET
__fadd_both_nz:
	; Align exponents: shift the smaller mantissa right
	LDA __fa_e1
	MOV B,A
	LDA __fa_e2
	CMP B
	JZ __fadd_aligned       ; equal exponents
	JNC __fadd_e2_bigger     ; e2 > e1
	; e1 > e2: shift m2 right by (e1 - e2) times
	SUB B                   ; A = e2 - e1 (negative)
	CMA
	INR A                   ; A = e1 - e2
	MOV C,A                 ; C = shift count
	CPI 25
	JNC __fadd_e1_dominates ; if shift >= 25, m2 becomes 0
__fadd_shift_m2:
	; Shift m2 right by 1
	LDA __fa_m2+3
	ORA A
	RAR
	STA __fa_m2+3
	LDA __fa_m2+2
	RAR
	STA __fa_m2+2
	LDA __fa_m2+1
	RAR
	STA __fa_m2+1
	LDA __fa_m2
	RAR
	STA __fa_m2
	DCR C
	JNZ __fadd_shift_m2
	; Set exponent to the larger one (e1)
	JMP __fadd_aligned
__fadd_e1_dominates:
	; m2 shifted to zero, result is op1
	LHLD __op1
	RET
__fadd_e2_bigger:
	; e2 > e1: shift m1 right by (e2 - e1) times
	LDA __fa_e2
	MOV B,A
	LDA __fa_e1
	MOV C,A
	MOV A,B
	SUB C                   ; A = e2 - e1
	MOV C,A
	CPI 25
	JNC __fadd_e2_dominates
__fadd_shift_m1:
	LDA __fa_m1+3
	ORA A
	RAR
	STA __fa_m1+3
	LDA __fa_m1+2
	RAR
	STA __fa_m1+2
	LDA __fa_m1+1
	RAR
	STA __fa_m1+1
	LDA __fa_m1
	RAR
	STA __fa_m1
	DCR C
	JNZ __fadd_shift_m1
	; Set result exponent to e2
	LDA __fa_e2
	STA __fa_e1
	JMP __fadd_aligned
__fadd_e2_dominates:
	; m1 shifted to zero, result is op2
	LDA __op2
	STA __op1
	LDA __op2+1
	STA __op1+1
	LDA __op2+2
	STA __op1+2
	LDA __op2+3
	STA __op1+3
	LHLD __op1
	RET

__fadd_aligned:
	; Signs match?
	LDA __fa_s1
	MOV B,A
	LDA __fa_s2
	CMP B
	JNZ __fadd_subtract
	; Same sign: add mantissas
	LDA __fa_m1
	MOV B,A
	LDA __fa_m2
	ADD B
	STA __fa_m1
	LDA __fa_m1+1
	MOV B,A
	LDA __fa_m2+1
	ADC B
	STA __fa_m1+1
	LDA __fa_m1+2
	MOV B,A
	LDA __fa_m2+2
	ADC B
	STA __fa_m1+2
	LDA __fa_m1+3
	MOV B,A
	LDA __fa_m2+3
	ADC B
	STA __fa_m1+3
	; Sign stays the same, normalize (may need right shift)
	CALL __fnorm
	CALL __fpack
	RET
__fadd_subtract:
	; Different signs: subtract smaller mantissa from larger
	; Compare mantissas (m1 vs m2) unsigned, 4 bytes
	LDA __fa_m1+3
	MOV B,A
	LDA __fa_m2+3
	CMP B
	JC __fadd_m1_bigger
	JNZ __fadd_m2_bigger
	LDA __fa_m1+2
	MOV B,A
	LDA __fa_m2+2
	CMP B
	JC __fadd_m1_bigger
	JNZ __fadd_m2_bigger
	LDA __fa_m1+1
	MOV B,A
	LDA __fa_m2+1
	CMP B
	JC __fadd_m1_bigger
	JNZ __fadd_m2_bigger
	LDA __fa_m1
	MOV B,A
	LDA __fa_m2
	CMP B
	JC __fadd_m1_bigger
	JNZ __fadd_m2_bigger
	; Equal mantissas with different signs -> result is +0
	XRA A
	STA __op1
	STA __op1+1
	STA __op1+2
	STA __op1+3
	LHLD __op1
	RET
__fadd_m1_bigger:
	; m1 >= m2: result sign = s1, m1 = m1 - m2
	LDA __fa_m1
	MOV B,A
	LDA __fa_m2
	MOV C,A
	MOV A,B
	SUB C
	STA __fa_m1
	LDA __fa_m1+1
	MOV B,A
	LDA __fa_m2+1
	MOV C,A
	MOV A,B
	SBB C
	STA __fa_m1+1
	LDA __fa_m1+2
	MOV B,A
	LDA __fa_m2+2
	MOV C,A
	MOV A,B
	SBB C
	STA __fa_m1+2
	LDA __fa_m1+3
	MOV B,A
	LDA __fa_m2+3
	MOV C,A
	MOV A,B
	SBB C
	STA __fa_m1+3
	; Sign remains s1
	CALL __fnorm
	CALL __fpack
	RET
__fadd_m2_bigger:
	; m2 > m1: result sign = s2, m1 = m2 - m1
	LDA __fa_s2
	STA __fa_s1             ; result gets s2 sign
	LDA __fa_m2
	MOV B,A
	LDA __fa_m1
	MOV C,A
	MOV A,B
	SUB C
	STA __fa_m1
	LDA __fa_m2+1
	MOV B,A
	LDA __fa_m1+1
	MOV C,A
	MOV A,B
	SBB C
	STA __fa_m1+1
	LDA __fa_m2+2
	MOV B,A
	LDA __fa_m1+2
	MOV C,A
	MOV A,B
	SBB C
	STA __fa_m1+2
	LDA __fa_m2+3
	MOV B,A
	LDA __fa_m1+3
	MOV C,A
	MOV A,B
	SBB C
	STA __fa_m1+3
	CALL __fnorm
	CALL __fpack
	RET

; =====================================================================
; __fsub — float subtract: __op1 = __op1 - __op2
;   Flip sign of op2 and add.
; =====================================================================
__fsub:
	; Flip sign bit of __op2
	LDA __op2+3
	XRI 080h
	STA __op2+3
	JMP __fadd

; =====================================================================
; __fmul — float multiply: __op1 = __op1 * __op2
; =====================================================================
__fmul:
	CALL __funpack1
	CALL __funpack2
	; Result sign = s1 XOR s2
	LDA __fa_s1
	MOV B,A
	LDA __fa_s2
	XRA B
	STA __fa_s1
	; Check for zero
	LDA __fa_e1
	ORA A
	JZ __fmul_zero
	LDA __fa_e2
	ORA A
	JZ __fmul_zero
	; Result exponent = e1 + e2 - 127
	LDA __fa_e1
	MOV B,A
	LDA __fa_e2
	ADD B
	SUI 127                 ; subtract bias
	STA __fa_e1
	; Multiply mantissas: m1 (24 bit) * m2 (24 bit) -> 48 bits
	; We only need the top 24 bits for the result.
	; Use shift-and-add: accumulate in __fa_tmp (6 bytes = 48 bits)
	; Clear accumulator
	XRA A
	STA __fa_tmp
	STA __fa_tmp+1
	STA __fa_tmp+2
	STA __fa_tmp+3
	STA __fa_tmp+4
	STA __fa_tmp+5
	; Loop over 24 bits of m2
	MVI D,24                ; bit counter
__fmul_loop:
	; Test low bit of m2
	LDA __fa_m2
	RAR
	JNC __fmul_noadd
	; Add m1 to upper 3 bytes of accumulator (tmp+3..tmp+5)
	LDA __fa_tmp+3
	MOV B,A
	LDA __fa_m1
	ADD B
	STA __fa_tmp+3
	LDA __fa_tmp+4
	MOV B,A
	LDA __fa_m1+1
	ADC B
	STA __fa_tmp+4
	LDA __fa_tmp+5
	MOV B,A
	LDA __fa_m1+2
	ADC B
	STA __fa_tmp+5
__fmul_noadd:
	; Shift accumulator right by 1 (48 bits)
	LDA __fa_tmp+5
	ORA A
	RAR
	STA __fa_tmp+5
	LDA __fa_tmp+4
	RAR
	STA __fa_tmp+4
	LDA __fa_tmp+3
	RAR
	STA __fa_tmp+3
	LDA __fa_tmp+2
	RAR
	STA __fa_tmp+2
	LDA __fa_tmp+1
	RAR
	STA __fa_tmp+1
	LDA __fa_tmp
	RAR
	STA __fa_tmp
	; Shift m2 right by 1
	LDA __fa_m2+2
	ORA A
	RAR
	STA __fa_m2+2
	LDA __fa_m2+1
	RAR
	STA __fa_m2+1
	LDA __fa_m2
	RAR
	STA __fa_m2
	DCR D
	JNZ __fmul_loop
	; Result mantissa is in tmp+3..tmp+5 (top 24 bits)
	LDA __fa_tmp+3
	STA __fa_m1
	LDA __fa_tmp+4
	STA __fa_m1+1
	LDA __fa_tmp+5
	STA __fa_m1+2
	XRA A
	STA __fa_m1+3
	; Normalize and pack
	CALL __fnorm
	CALL __fpack
	RET
__fmul_zero:
	XRA A
	STA __op1
	STA __op1+1
	STA __op1+2
	STA __op1+3
	LHLD __op1
	RET

; =====================================================================
; __fdiv — float divide: __op1 = __op1 / __op2
; =====================================================================
__fdiv:
	CALL __funpack1
	CALL __funpack2
	; Result sign = s1 XOR s2
	LDA __fa_s1
	MOV B,A
	LDA __fa_s2
	XRA B
	STA __fa_s1
	; Check for zero dividend
	LDA __fa_e1
	ORA A
	JZ __fdiv_zero
	; Check for zero divisor (return infinity-ish, saturate exponent)
	LDA __fa_e2
	ORA A
	JZ __fdiv_inf
	; Result exponent = e1 - e2 + 127
	LDA __fa_e1
	MOV B,A
	LDA __fa_e2
	MOV C,A
	MOV A,B
	SUB C
	ADI 127                 ; add bias
	STA __fa_e1
	; Divide mantissas: m1 / m2, producing 24-bit quotient
	; Use restoring division: shift dividend left, subtract divisor,
	; quotient bits shift into result.
	; Clear quotient in __fa_tmp (3 bytes)
	XRA A
	STA __fa_tmp
	STA __fa_tmp+1
	STA __fa_tmp+2
	; We also need a 4-byte remainder (m1 acts as remainder, extended)
	STA __fa_m1+3           ; ensure byte 3 is 0
	MVI D,24                ; 24 bits to produce
__fdiv_loop:
	; Shift remainder (m1, 4 bytes) left by 1
	LDA __fa_m1
	ADD A
	STA __fa_m1
	LDA __fa_m1+1
	ADC A
	STA __fa_m1+1
	LDA __fa_m1+2
	ADC A
	STA __fa_m1+2
	LDA __fa_m1+3
	ADC A
	STA __fa_m1+3
	; Shift quotient left by 1
	LDA __fa_tmp
	ADD A
	STA __fa_tmp
	LDA __fa_tmp+1
	ADC A
	STA __fa_tmp+1
	LDA __fa_tmp+2
	ADC A
	STA __fa_tmp+2
	; Compare remainder (top 3 bytes: m1+1..m1+3) >= divisor (m2, 3 bytes)
	; Compare m1+3 vs 0 (m2 has only 3 bytes, so byte 3 of m2=0)
	LDA __fa_m1+3
	ORA A
	JNZ __fdiv_subtract     ; if remainder byte 3 > 0, definitely >=
	LDA __fa_m1+2
	MOV B,A
	LDA __fa_m2+2
	CMP B
	JC __fdiv_subtract       ; m2+2 < m1+2 -> remainder > divisor
	JNZ __fdiv_nosub         ; m2+2 > m1+2 -> remainder < divisor
	LDA __fa_m1+1
	MOV B,A
	LDA __fa_m2+1
	CMP B
	JC __fdiv_subtract
	JNZ __fdiv_nosub
	LDA __fa_m1
	MOV B,A
	LDA __fa_m2
	CMP B
	JC __fdiv_subtract
	JNZ __fdiv_nosub
	; Equal -> subtract
__fdiv_subtract:
	; remainder -= divisor
	LDA __fa_m1
	MOV B,A
	LDA __fa_m2
	MOV C,A
	MOV A,B
	SUB C
	STA __fa_m1
	LDA __fa_m1+1
	MOV B,A
	LDA __fa_m2+1
	MOV C,A
	MOV A,B
	SBB C
	STA __fa_m1+1
	LDA __fa_m1+2
	MOV B,A
	LDA __fa_m2+2
	MOV C,A
	MOV A,B
	SBB C
	STA __fa_m1+2
	LDA __fa_m1+3
	SBI 0
	STA __fa_m1+3
	; Set quotient bit
	LDA __fa_tmp
	ORI 1
	STA __fa_tmp
__fdiv_nosub:
	DCR D
	JNZ __fdiv_loop
	; Quotient is in __fa_tmp (3 bytes) -> move to m1
	LDA __fa_tmp
	STA __fa_m1
	LDA __fa_tmp+1
	STA __fa_m1+1
	LDA __fa_tmp+2
	STA __fa_m1+2
	XRA A
	STA __fa_m1+3
	CALL __fnorm
	CALL __fpack
	RET
__fdiv_zero:
	XRA A
	STA __op1
	STA __op1+1
	STA __op1+2
	STA __op1+3
	LHLD __op1
	RET
__fdiv_inf:
	; Return "infinity" (max exponent, zero mantissa) with correct sign
	LDA __fa_s1
	ORA A
	JNZ __fdiv_inf_neg
	MVI A,07Fh
	STA __op1+3
	MVI A,080h
	STA __op1+2
	XRA A
	STA __op1+1
	STA __op1
	LHLD __op1
	RET
__fdiv_inf_neg:
	MVI A,0FFh
	STA __op1+3
	MVI A,080h
	STA __op1+2
	XRA A
	STA __op1+1
	STA __op1
	LHLD __op1
	RET

; =====================================================================
; __feq — float equal: HL = (__op1 == __op2) ? 1 : 0
; =====================================================================
__feq:
	; Special case: +0 == -0
	CALL __fiszero1
	JNZ __feq_check
	CALL __fiszero2
	JNZ __feq_check
	; Both zero
	LXI H,1
	RET
__feq_check:
	; Compare all 4 bytes
	LDA __op1
	MOV B,A
	LDA __op2
	CMP B
	JNZ __feq_no
	LDA __op1+1
	MOV B,A
	LDA __op2+1
	CMP B
	JNZ __feq_no
	LDA __op1+2
	MOV B,A
	LDA __op2+2
	CMP B
	JNZ __feq_no
	LDA __op1+3
	MOV B,A
	LDA __op2+3
	CMP B
	JNZ __feq_no
	LXI H,1
	RET
__feq_no:
	LXI H,0
	RET

; =====================================================================
; __fne — float not equal: HL = (__op1 != __op2) ? 1 : 0
; =====================================================================
__fne:
	CALL __feq
	MOV A,L
	XRI 1
	MOV L,A
	RET

; =====================================================================
; __fiszero1 — check if __op1 is zero (NZ if nonzero, Z if zero)
; =====================================================================
__fiszero1:
	LDA __op1
	MOV B,A
	LDA __op1+1
	ORA B
	MOV B,A
	LDA __op1+2
	ORA B
	MOV B,A
	LDA __op1+3
	ANI 07Fh                ; ignore sign bit
	ORA B
	RET

; =====================================================================
; __fiszero2 — check if __op2 is zero (NZ if nonzero, Z if zero)
; =====================================================================
__fiszero2:
	LDA __op2
	MOV B,A
	LDA __op2+1
	ORA B
	MOV B,A
	LDA __op2+2
	ORA B
	MOV B,A
	LDA __op2+3
	ANI 07Fh
	ORA B
	RET

; =====================================================================
; __flt — float less than: HL = (__op1 < __op2) ? 1 : 0
; =====================================================================
__flt:
	; Handle zeros
	CALL __fiszero1
	JNZ __flt_op1nz
	CALL __fiszero2
	JNZ __flt_op2nz_op1z
	; Both zero -> not less
	LXI H,0
	RET
__flt_op2nz_op1z:
	; op1=0, op2!=0: op1 < op2 iff op2 is positive
	LDA __op2+3
	ANI 080h
	JNZ __flt_false         ; op2 is negative, 0 is not less
	LXI H,1
	RET
__flt_op1nz:
	; op1 is nonzero
	; Compare signs first
	LDA __op1+3
	ANI 080h
	MOV B,A                 ; B = sign1
	LDA __op2+3
	ANI 080h
	MOV C,A                 ; C = sign2
	; If sign1 != sign2: negative < positive
	MOV A,B
	CMP C
	JZ __flt_samesign
	; Different signs: op1 < op2 iff op1 is negative (sign1=0x80)
	MOV A,B
	ORA A
	JZ __flt_false          ; op1 positive, op2 negative
	LXI H,1                 ; op1 negative, op2 positive
	RET
__flt_samesign:
	; Same sign: compare magnitude (bytes 3,2,1,0 as big-endian unsigned)
	; For positive: smaller magnitude = smaller number
	; For negative: smaller magnitude = larger number (negate result)
	MOV A,B                 ; sign (0 or 0x80)
	PUSH PSW                ; save sign
	; Compare byte 3 (ignore sign bit for magnitude)
	LDA __op1+3
	ANI 07Fh
	MOV B,A
	LDA __op2+3
	ANI 07Fh
	CMP B
	JC __flt_mag1bigger
	JNZ __flt_mag2bigger
	; byte 2
	LDA __op1+2
	MOV B,A
	LDA __op2+2
	CMP B
	JC __flt_mag1bigger
	JNZ __flt_mag2bigger
	; byte 1
	LDA __op1+1
	MOV B,A
	LDA __op2+1
	CMP B
	JC __flt_mag1bigger
	JNZ __flt_mag2bigger
	; byte 0
	LDA __op1
	MOV B,A
	LDA __op2
	CMP B
	JC __flt_mag1bigger
	JNZ __flt_mag2bigger
	; Equal magnitude -> not less than
	POP PSW
	LXI H,0
	RET
__flt_mag1bigger:
	; |op1| > |op2|
	POP PSW                 ; A = sign
	ORA A
	JZ __flt_false          ; positive: bigger mag -> not less
	LXI H,1                 ; negative: bigger mag -> less
	RET
__flt_mag2bigger:
	; |op2| > |op1|
	POP PSW
	ORA A
	JZ __flt_true           ; positive: smaller mag -> less
	LXI H,0                 ; negative: smaller mag -> not less
	RET
__flt_true:
	LXI H,1
	RET
__flt_false:
	LXI H,0
	RET

; =====================================================================
; __fle — float less or equal: HL = (__op1 <= __op2) ? 1 : 0
; =====================================================================
__fle:
	; a <= b  iff  !(b < a)
	; Swap op1 and op2, call __flt, negate
	; Save op1 to tmp
	LDA __op1
	STA __fa_tmp
	LDA __op1+1
	STA __fa_tmp+1
	LDA __op1+2
	STA __fa_tmp+2
	LDA __op1+3
	STA __fa_tmp+3
	; Copy op2 to op1
	LDA __op2
	STA __op1
	LDA __op2+1
	STA __op1+1
	LDA __op2+2
	STA __op1+2
	LDA __op2+3
	STA __op1+3
	; Copy saved op1 to op2
	LDA __fa_tmp
	STA __op2
	LDA __fa_tmp+1
	STA __op2+1
	LDA __fa_tmp+2
	STA __op2+2
	LDA __fa_tmp+3
	STA __op2+3
	; Now __flt computes (old_op2 < old_op1)
	CALL __flt
	; Negate: a <= b iff !(b < a)
	MOV A,L
	XRI 1
	MOV L,A
	RET

; =====================================================================
; __fgt — float greater than: HL = (__op1 > __op2) ? 1 : 0
; =====================================================================
__fgt:
	; a > b  iff  b < a: swap and call __flt
	LDA __op1
	STA __fa_tmp
	LDA __op1+1
	STA __fa_tmp+1
	LDA __op1+2
	STA __fa_tmp+2
	LDA __op1+3
	STA __fa_tmp+3
	LDA __op2
	STA __op1
	LDA __op2+1
	STA __op1+1
	LDA __op2+2
	STA __op1+2
	LDA __op2+3
	STA __op1+3
	LDA __fa_tmp
	STA __op2
	LDA __fa_tmp+1
	STA __op2+1
	LDA __fa_tmp+2
	STA __op2+2
	LDA __fa_tmp+3
	STA __op2+3
	CALL __flt
	RET

; =====================================================================
; __fge — float greater or equal: HL = (__op1 >= __op2) ? 1 : 0
; =====================================================================
__fge:
	; a >= b  iff  !(a < b)
	CALL __flt
	MOV A,L
	XRI 1
	MOV L,A
	RET

; =====================================================================
; __itof — convert signed int16 (in HL) to float in __op1
; =====================================================================
__itof:
	; Save sign
	MOV A,H
	ANI 080h
	STA __fa_s1
	; If HL is 0, return 0.0
	MOV A,H
	ORA L
	JNZ __itof_nz
	XRA A
	STA __op1
	STA __op1+1
	STA __op1+2
	STA __op1+3
	LHLD __op1
	RET
__itof_nz:
	; If negative, negate
	LDA __fa_s1
	ORA A
	JZ __itof_pos
	; Negate HL: HL = 0 - HL
	MOV A,L
	CMA
	MOV L,A
	MOV A,H
	CMA
	MOV H,A
	INX H
__itof_pos:
	; Store magnitude in m1 (low 16 bits)
	MOV A,L
	STA __fa_m1
	MOV A,H
	STA __fa_m1+1
	XRA A
	STA __fa_m1+2
	STA __fa_m1+3
	; Start with exponent = 127 + 23 = 150 (we'll shift mantissa
	; to place the MSB at bit 23)
	MVI A,150
	STA __fa_e1
	; The value is in bits 0..15 of m1. We need the MSB at bit 23.
	; Shift left by 8 to get it into bits 8..23.
	LDA __fa_m1+1
	STA __fa_m1+2
	LDA __fa_m1
	STA __fa_m1+1
	XRA A
	STA __fa_m1
	; Now value is in bits 8..23. Adjust exponent: we shifted left 8,
	; so subtract 8 from exponent.
	LDA __fa_e1
	SUI 8
	STA __fa_e1
	; Normalize: shift left until bit 23 is set, decrementing exponent
	CALL __fnorm
	CALL __fpack
	RET

; =====================================================================
; __ftoi — convert float in __op1 to signed int16 in HL
; =====================================================================
__ftoi:
	CALL __funpack1
	; If exponent is 0 (or mantissa is 0), return 0
	LDA __fa_e1
	ORA A
	JZ __ftoi_zero
	; The mantissa has implicit 1 at bit 23.
	; The actual value = mantissa * 2^(exponent - 127 - 23)
	; We need to shift mantissa right by (150 - exponent) or left if exp > 150.
	; For int16, we only care about bits that end up in the low 16 range.
	LDA __fa_e1
	SUI 150                 ; A = exp - 150
	JP __ftoi_shift_left
	; exp < 150: shift mantissa right by (150 - exp)
	CMA
	INR A                   ; A = 150 - exp = shift right count
	MOV C,A
	CPI 24
	JNC __ftoi_zero         ; shift >= 24 -> result is 0
__ftoi_shr:
	; Shift m1 right by 1
	LDA __fa_m1+2
	ORA A
	RAR
	STA __fa_m1+2
	LDA __fa_m1+1
	RAR
	STA __fa_m1+1
	LDA __fa_m1
	RAR
	STA __fa_m1
	DCR C
	JNZ __ftoi_shr
	JMP __ftoi_result
__ftoi_shift_left:
	; exp > 150: shift mantissa left (value is large)
	MOV C,A
	ORA A
	JZ __ftoi_result
	CPI 16
	JNC __ftoi_overflow     ; too large for int16
__ftoi_shl:
	LDA __fa_m1
	ADD A
	STA __fa_m1
	LDA __fa_m1+1
	ADC A
	STA __fa_m1+1
	LDA __fa_m1+2
	ADC A
	STA __fa_m1+2
	DCR C
	JNZ __ftoi_shl
__ftoi_result:
	; Result is in m1 low 16 bits
	LDA __fa_m1+1
	MOV H,A
	LDA __fa_m1
	MOV L,A
	; Apply sign
	LDA __fa_s1
	ORA A
	RZ                      ; positive, done
	; Negate HL
	MOV A,L
	CMA
	MOV L,A
	MOV A,H
	CMA
	MOV H,A
	INX H
	RET
__ftoi_zero:
	LXI H,0
	RET
__ftoi_overflow:
	; Return max int16 with appropriate sign
	LDA __fa_s1
	ORA A
	JNZ __ftoi_neg_overflow
	LXI H,7FFFh            ; INT16_MAX
	RET
__ftoi_neg_overflow:
	LXI H,8000h            ; INT16_MIN
	RET

; =====================================================================
; Scratch data storage for float operations
; =====================================================================
__fa_s1:
	.storage 1
__fa_e1:
	.storage 1
__fa_m1:
	.storage 4
__fa_s2:
	.storage 1
__fa_e2:
	.storage 1
__fa_m2:
	.storage 4
__fa_tmp:
	.storage 8
