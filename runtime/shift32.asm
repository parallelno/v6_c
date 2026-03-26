; shift32.asm — 32-bit shift routines for Intel 8080
;
; __shl32:   __op1 = __op1 << B  (logical left shift)
; __shr32u:  __op1 = __op1 >> B  (logical right shift, unsigned)
; __shr32s:  __op1 = __op1 >> B  (arithmetic right shift, signed)
;
; Entry:  __op1 (4 bytes) = value, B = shift count
; Exit:   __op1 (4 bytes) = shifted result, HL = low 16 bits
; Clobbers: A, B, flags

; ---------------------------------------------------------------------------
; __shl32 — logical left shift:  __op1 <<= B
; ---------------------------------------------------------------------------
__shl32:
	MOV A,B
	ANI 0x1F		; clamp to 0..31
	RZ			; shift by 0 → no-op
	MOV B,A
__shl32_loop:
	LDA __op1
	ADD A
	STA __op1
	LDA __op1+1
	ADC A
	STA __op1+1
	LDA __op1+2
	ADC A
	STA __op1+2
	LDA __op1+3
	ADC A
	STA __op1+3
	DCR B
	JNZ __shl32_loop
	LHLD __op1
	RET

; ---------------------------------------------------------------------------
; __shr32u — logical right shift (unsigned):  __op1 >>= B
; ---------------------------------------------------------------------------
__shr32u:
	MOV A,B
	ANI 0x1F
	RZ
	MOV B,A
__shr32u_loop:
	LDA __op1+3
	ORA A		; clear carry (zero into MSB)
	RAR
	STA __op1+3
	LDA __op1+2
	RAR
	STA __op1+2
	LDA __op1+1
	RAR
	STA __op1+1
	LDA __op1
	RAR
	STA __op1
	DCR B
	JNZ __shr32u_loop
	LHLD __op1
	RET

; ---------------------------------------------------------------------------
; __shr32s — arithmetic right shift (signed):  __op1 >>= B
; ---------------------------------------------------------------------------
__shr32s:
	MOV A,B
	ANI 0x1F
	RZ
	MOV B,A
__shr32s_loop:
	LDA __op1+3
	RAL			; carry = sign bit
	LDA __op1+3
	RAR			; shift right preserving sign
	STA __op1+3
	LDA __op1+2
	RAR
	STA __op1+2
	LDA __op1+1
	RAR
	STA __op1+1
	LDA __op1
	RAR
	STA __op1
	DCR B
	JNZ __shr32s_loop
	LHLD __op1
	RET
