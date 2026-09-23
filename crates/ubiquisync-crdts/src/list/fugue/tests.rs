use rand::seq::SliceRandom;

use super::*;

#[test]
fn reference_test() {
    let mut list = List::<usize, usize>::default();
    let mut v = Vec::<usize>::default();
    (0..100).into_iter().for_each(|i| {
        v.push(i);
    });
    let mut ops = v.clone();
    let mut rng = rand::rng();
    ops.shuffle(&mut rng);
    for op in ops {
        let insert = list.create_insert(View::Effect, op, vec![op]);
        list.apply_op(Op::Insert {
            id: ElementId {
                op_id: op,
                index: 0,
            },
            insert,
        })
        .unwrap()
    }
    assert_eq!(v, list.iter().copied().collect::<Vec<_>>())
}
