; shift.asm — 16-bit shift routines for Intel 8080
;
; __shl16:   HL = HL << A  (logical left shift)
; __shr16u:  HL = HL >> A  (logical right shift, unsigned)
; __shr16s:  HL = HL >> A  (arithmetic right shift, signed)
;
; Entry:  HL = value, A = shift count (must be > 0)
; Exit:   HL = shifted result
; Clobbers: A, flags

; ---------------------------------------------------------------------------
; __shl16 — logical left shift:  HL <<= A
; ---------------------------------------------------------------------------
__shl16:
	DAD H			; HL <<= 1
	DCR A
	JNZ __shl16
	RET

; ---------------------------------------------------------------------------
; __shr16u — logical right shift (unsigned):  HL >>= A
; ---------------------------------------------------------------------------
__shr16u:
	PUSH PSW		; save counter
	MOV A,H
	ORA A			; clear carry (zero into MSB)
	RAR
	MOV H,A
	MOV A,L
	RAR
	MOV L,A
	POP PSW			; restore counter
	DCR A
	JNZ __shr16u
	RET

; ---------------------------------------------------------------------------
; __shr16s — arithmetic right shift (signed):  HL >>= A
; ---------------------------------------------------------------------------
__shr16s:
	PUSH PSW		; save counter
	MOV A,H
	RAL			; carry = sign bit
	MOV A,H
	RAR			; shift right preserving sign
	MOV H,A
	MOV A,L
	RAR
	MOV L,A
	POP PSW			; restore counter
	DCR A
	JNZ __shr16s
	RET
