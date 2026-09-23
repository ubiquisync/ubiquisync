use rand::{
    RngExt,
    distr::{Alphanumeric, SampleString},
};

use super::*;

// #[test]
// fn reference_test() {
//     let mut list = List::<usize, usize>::default();
//     let mut v = Vec::<usize>::default();
//     (0..100).into_iter().for_each(|i| {
//         v.push(i);
//     });
//     let mut ops = v.clone();
//     let mut rng = rand::rng();
//     ops.shuffle(&mut rng);
//     for op in ops {
//         let insert = list.create_insert(View::Effect, op, vec![op]);
//         list.apply_op(Op::Insert {
//             id: ElementId {
//                 op_id: op,
//                 index: 0,
//             },
//             insert,
//         })
//         .unwrap()
//     }
//     assert_eq!(v, list.iter().copied().collect::<Vec<_>>())
// }

#[test]
fn sim_test() {
    let mut rng = rand::rng();
    const NUM_PEERS: usize = 5;
    const ROUNDS: usize = 200;
    let mut lists: [List<(usize, usize), char>; NUM_PEERS] = Default::default();
    let mut oplog: [Vec<Op<(usize, usize), char>>; NUM_PEERS] = Default::default();
    let mut frontiers: [[usize; NUM_PEERS]; NUM_PEERS] = Default::default();
    for _ in 0..ROUNDS {
        let peer = rng.random_range(0..NUM_PEERS);
        let list = &mut lists[peer];
        let size = list.size(View::Effect);
        let offset = if size == 0 {
            0
        } else {
            rng.random_range(0..size)
        };

        // TODO also do deletes
        let content_size = rng.random_range(0..16);
        let content = Alphanumeric.sample_string(&mut rng, content_size);
        let insert = list.create_insert(View::Effect, offset, content.chars().collect());
        let op_idx = oplog[peer].len();
        let id = ElementId {
            op_id: (peer, op_idx),
            index: 0,
        };
        let op = Op::Insert { id, insert };
        list.apply_op(op.clone()).unwrap();
        oplog[peer].push(op);
        frontiers[peer][peer] = op_idx + 1;

        // advance each peer's frontier by a random amount
        // to simulate inconsistent intermediate states
        for i in 0..NUM_PEERS {
            for j in 0..NUM_PEERS {
                if i == j {
                    // skip trying to echo self
                    continue;
                }

                let cur_read_size = frontiers[i][j];
                let available_size = oplog[j].len();
                if cur_read_size == available_size {
                    continue;
                }
                let new_read_size = rng.random_range(cur_read_size..=available_size);
                let list = &mut lists[i];
                for op in oplog[j][cur_read_size..new_read_size].iter() {
                    list.apply_op(op.clone()).unwrap();
                }
                frontiers[i][j] = new_read_size;
            }
        }
    }

    // at the advance all peer frontiers to the end
    for i in 0..NUM_PEERS {
        for j in 0..NUM_PEERS {
            if i == j {
                // skip trying to echo self
                continue;
            }

            let cur_read_size = frontiers[i][j];
            for op in oplog[j][cur_read_size..].iter() {
                lists[i].apply_op(op.clone()).unwrap();
            }
        }
    }

    // confirm all peers materialized identical state
    let strings = lists.map(|l| l.iter().collect::<String>());
    for i in 1..NUM_PEERS {
        assert_eq!(strings[0], strings[i]);
    }
}
