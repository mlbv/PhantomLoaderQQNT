# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

.equ FRAME_REGISTERS, {registers}
.equ FRAME_PADDING, {padding}
.equ FRAME_XMM, {xmm}
.equ FRAME_STACK, {stack}
.equ FRAME_STACK_BYTES, {stack_bytes}
.equ FRAME_TOKEN, {token}
.equ FRAME_USER_DATA, {user_data}
.equ FRAME_SIZE, {frame_size}
.equ EXECUTION_SIZE, {execution_size}
.equ EXECUTION_CALLBACK, {execution_callback}
.equ EXECUTION_TARGET, {execution_target}

.equ ENTRY_FRAME, 32
.equ ENTRY_EXECUTION, ENTRY_FRAME + FRAME_SIZE
.equ ENTRY_LEASE, ENTRY_EXECUTION + EXECUTION_SIZE
.equ ENTRY_LAST_ERROR, ENTRY_LEASE + 8
.equ ENTRY_CALLEE, ENTRY_LAST_ERROR + 8
.equ ENTRY_SIZE, ENTRY_CALLEE + 8

.equ ORIGINAL_EXECUTION, 32
.equ ORIGINAL_LEASE, ORIGINAL_EXECUTION + EXECUTION_SIZE
.equ ORIGINAL_FRAME, ORIGINAL_LEASE + 8
.equ ORIGINAL_LAST_ERROR, ORIGINAL_FRAME + 8
.equ ORIGINAL_SIZE, ORIGINAL_LAST_ERROR + 24

.equ TARGET_CALLEE, 32
.equ TARGET_SIZE, 40

.macro dispatch_save base, offset
    mov [\base + \offset + FRAME_REGISTERS + 0], rax
    mov [\base + \offset + FRAME_REGISTERS + 8], rcx
    mov [\base + \offset + FRAME_REGISTERS + 16], rdx
    mov [\base + \offset + FRAME_REGISTERS + 24], r8
    mov [\base + \offset + FRAME_REGISTERS + 32], r9
    mov [\base + \offset + FRAME_REGISTERS + 40], r10
    mov [\base + \offset + FRAME_REGISTERS + 48], r11
    movdqu [\base + \offset + FRAME_XMM + 0], xmm0
    movdqu [\base + \offset + FRAME_XMM + 16], xmm1
    movdqu [\base + \offset + FRAME_XMM + 32], xmm2
    movdqu [\base + \offset + FRAME_XMM + 48], xmm3
    movdqu [\base + \offset + FRAME_XMM + 64], xmm4
    movdqu [\base + \offset + FRAME_XMM + 80], xmm5
.endm

.macro dispatch_restore base, offset
    mov rax, [\base + \offset + FRAME_REGISTERS + 0]
    mov rcx, [\base + \offset + FRAME_REGISTERS + 8]
    mov rdx, [\base + \offset + FRAME_REGISTERS + 16]
    mov r8, [\base + \offset + FRAME_REGISTERS + 24]
    mov r9, [\base + \offset + FRAME_REGISTERS + 32]
    mov r10, [\base + \offset + FRAME_REGISTERS + 40]
    mov r11, [\base + \offset + FRAME_REGISTERS + 48]
    movdqu xmm0, [\base + \offset + FRAME_XMM + 0]
    movdqu xmm1, [\base + \offset + FRAME_XMM + 16]
    movdqu xmm2, [\base + \offset + FRAME_XMM + 32]
    movdqu xmm3, [\base + \offset + FRAME_XMM + 48]
    movdqu xmm4, [\base + \offset + FRAME_XMM + 64]
    movdqu xmm5, [\base + \offset + FRAME_XMM + 80]
.endm

.macro dispatch_restore_error offset
    mov r11d, [rsp + \offset]
    mov gs:[0x68], r11d
.endm

.text
.p2align 4
.globl managed_entry
.def managed_entry
.scl 2
.type 32
.endef
.seh_proc managed_entry
managed_entry:
    sub rsp, ENTRY_SIZE
    .seh_stackalloc ENTRY_SIZE
    mov qword ptr [rsp + ENTRY_LEASE], 0
    .seh_endprologue

    dispatch_save rsp, ENTRY_FRAME
    mov eax, gs:[0x68]
    mov [rsp + ENTRY_LAST_ERROR], eax
    mov qword ptr [rsp + ENTRY_FRAME + FRAME_PADDING], 0
    lea rax, [rsp + ENTRY_SIZE + 40]
    mov [rsp + ENTRY_FRAME + FRAME_STACK], rax
    mov qword ptr [rsp + ENTRY_FRAME + FRAME_STACK_BYTES], 0
    mov qword ptr [rsp + ENTRY_FRAME + FRAME_TOKEN], 0
    mov qword ptr [rsp + ENTRY_FRAME + FRAME_USER_DATA], 0

    mov rcx, [rsp + ENTRY_FRAME + FRAME_REGISTERS + 40]
    lea rdx, [rsp + ENTRY_FRAME]
    lea r8, [rsp + ENTRY_EXECUTION]
    call {managed_enter}
    mov [rsp + ENTRY_LEASE], rax
    dispatch_restore_error ENTRY_LAST_ERROR
    mov rax, [rsp + ENTRY_EXECUTION + EXECUTION_CALLBACK]
    mov [rsp + ENTRY_CALLEE], rax
    test rax, rax
    jz .Lentry_bypass

    lea rcx, [rsp + ENTRY_FRAME]
    call qword ptr [rsp + ENTRY_CALLEE]
    jmp .Lentry_release

.Lentry_bypass:
    lea rcx, [rsp + ENTRY_FRAME]
    mov rdx, [rsp + ENTRY_EXECUTION + EXECUTION_TARGET]
    call managed_dispatch_call_target

.Lentry_release:
    mov eax, gs:[0x68]
    mov [rsp + ENTRY_LAST_ERROR], eax
    mov rcx, [rsp + ENTRY_LEASE]
    mov qword ptr [rsp + ENTRY_LEASE], 0
    call {managed_leave}
    dispatch_restore_error ENTRY_LAST_ERROR
    dispatch_restore rsp, ENTRY_FRAME
    add rsp, ENTRY_SIZE
    ret

    .seh_handler {managed_dispatch_unwind}, @unwind
    .seh_handlerdata
    .long ENTRY_LEASE
    .text
.seh_endproc

.p2align 4
.globl managed_invoke_original
.def managed_invoke_original
.scl 2
.type 32
.endef
.seh_proc managed_invoke_original
managed_invoke_original:
    sub rsp, ORIGINAL_SIZE
    .seh_stackalloc ORIGINAL_SIZE
    mov qword ptr [rsp + ORIGINAL_LEASE], 0
    .seh_endprologue

    mov [rsp + ORIGINAL_FRAME], rdx
    mov eax, gs:[0x68]
    mov [rsp + ORIGINAL_LAST_ERROR], eax
    lea r8, [rsp + ORIGINAL_EXECUTION]
    call {managed_original_enter}
    mov [rsp + ORIGINAL_LEASE], rax
    dispatch_restore_error ORIGINAL_LAST_ERROR
    test rax, rax
    jz .Loriginal_invalid
    mov rcx, [rsp + ORIGINAL_FRAME]
    mov rdx, [rsp + ORIGINAL_EXECUTION + EXECUTION_TARGET]
    call managed_dispatch_call_target

    mov eax, gs:[0x68]
    mov [rsp + ORIGINAL_LAST_ERROR], eax
    mov rcx, [rsp + ORIGINAL_LEASE]
    mov qword ptr [rsp + ORIGINAL_LEASE], 0
    call {managed_leave}
    dispatch_restore_error ORIGINAL_LAST_ERROR
    xor eax, eax
    add rsp, ORIGINAL_SIZE
    ret

.Loriginal_invalid:
    mov eax, 1
    add rsp, ORIGINAL_SIZE
    ret

    .seh_handler {managed_dispatch_unwind}, @unwind
    .seh_handlerdata
    .long ORIGINAL_LEASE
    .text
.seh_endproc

.p2align 4
.def managed_dispatch_call_target
.scl 3
.type 32
.endef
.seh_proc managed_dispatch_call_target
managed_dispatch_call_target:
    push rbp
    .seh_pushreg rbp
    push rbx
    .seh_pushreg rbx
    push rsi
    .seh_pushreg rsi
    push rdi
    .seh_pushreg rdi
    sub rsp, TARGET_SIZE
    .seh_stackalloc TARGET_SIZE
    mov rbp, rsp
    .seh_setframe rbp, 0
    .seh_endprologue

    mov rbx, rcx
    mov [rbp + TARGET_CALLEE], rdx
    mov rax, [rbx + FRAME_STACK_BYTES]
    add rax, 15
    and rax, -16
    add rax, 32
.Ltarget_probe:
    cmp rax, 4096
    jb .Ltarget_probe_tail
    sub rsp, 4096
    test byte ptr [rsp], 0
    sub rax, 4096
    jmp .Ltarget_probe

.Ltarget_probe_tail:
    sub rsp, rax
    test byte ptr [rsp], 0
    mov rsi, [rbx + FRAME_STACK]
    lea rdi, [rsp + 32]
    mov rcx, [rbx + FRAME_STACK_BYTES]
    rep movsb
    dispatch_restore rbx, 0
    call qword ptr [rbp + TARGET_CALLEE]
    dispatch_save rbx, 0

    lea rsp, [rbp + TARGET_SIZE]
    pop rdi
    pop rsi
    pop rbx
    pop rbp
    ret
.seh_endproc

.purgem dispatch_save
.purgem dispatch_restore
.purgem dispatch_restore_error

.data
.p2align 2
.globl _fltused
_fltused:
    .long 0x9875
