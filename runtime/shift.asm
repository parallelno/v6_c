; shift.asm — 16-bit shift routines for Intel 8080
;
; __shl16:   HL = HL << A  (logical left shift)
; __shr16u:  HL = HL >> A  (logical right shift, unsigned)
; __shr16s:  HL = HL >> A  (arithmetic right shift, signed)
;
; Entry:  HL = value, A = shift count
; Exit:   HL = shifted result
; Clobbers: A, B, flags

; ---------------------------------------------------------------------------
; __shl16 — logical left shift:  HL <<= A
; ---------------------------------------------------------------------------
__shl16:
	ANI 0x0F		; clamp to 0..15
	RZ			; shift by 0 → no-op
	MOV B,A			; B = counter
__shl16_loop:
	DAD H			; HL <<= 1
	DCR B
	JNZ __shl16_loop
	RET

; ---------------------------------------------------------------------------
; __shr16u — logical right shift (unsigned):  HL >>= A
; ---------------------------------------------------------------------------
__shr16u:
	ANI 0x0F
	RZ
	MOV B,A			; B = counter
__shr16u_loop:
	MOV A,H
	ORA A			; clear carry (zero into MSB)
	RAR
	MOV H,A
	MOV A,L
	RAR
	MOV L,A
	DCR B
	JNZ __shr16u_loop
	RET

; ---------------------------------------------------------------------------
; __shr16s — arithmetic right shift (signed):  HL >>= A
; ---------------------------------------------------------------------------
__shr16s:
	ANI 0x0F
	RZ
	MOV B,A			; B = counter
__shr16s_loop:
	MOV A,H
	RAL			; carry = sign bit
	MOV A,H
	RAR			; shift right preserving sign
	MOV H,A
	MOV A,L
	RAR
	MOV L,A
	DCR B
	JNZ __shr16s_loop
	RET
