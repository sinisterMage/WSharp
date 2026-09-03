// `break`, `continue`, and a continue expression.
// expect: 4
// expect: 10
fn main() i64 {
    var i: i64 = 0;
    var total = 0;
    while (i < 4) : (i += 1) {
        if (i == 2) { continue; }
        total = total + i;
    }
    print_int(total);

    var j: i64 = 0;
    var sum = 0;
    while (true) {
        if (j > 4) { break; }
        sum = sum + j;
        j += 1;
    }
    print_int(sum);
    return 0;
}
