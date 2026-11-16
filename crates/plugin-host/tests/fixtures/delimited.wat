(module
  (memory (export "memory") 1)
  (global $open (mut i32) (i32.const 0))

  (func (export "ms_abi_version") (result i32)
    i32.const 1)

  (func (export "ms_reserve_input") (param i32) (result i32)
    i32.const 0)

  (func (export "ms_step")
    (param $phase i32) (param $ptr i32) (param $len i32) (result i64)
    (local $i i32)
    (if (i32.eqz (local.get $phase))
      (then
        (global.set $open (i32.const 0))
        (if (i32.eqz (local.get $len)) (then (return (i64.const 0))))
        (if (i32.ne (i32.load8_u (local.get $ptr)) (i32.const 64))
          (then (return (i64.const 0))))
        (global.set $open (i32.const 1))
        (local.set $i (i32.const 1))))
    (if (i32.eqz (global.get $open)) (then (return (i64.const 0))))
    (block $end
      (loop $next
        (br_if $end (i32.ge_u (local.get $i) (local.get $len)))
        (if (i32.eq
              (i32.load8_u (i32.add (local.get $ptr) (local.get $i)))
              (i32.const 33))
          (then
            (global.set $open (i32.const 0))
            (return
              (i64.or (i64.const 8589934592)
                (i64.extend_i32_u (i32.add (local.get $i) (i32.const 1)))))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $next)))
    (i64.or (i64.const 4294967296) (i64.extend_i32_u (local.get $len))))
)
