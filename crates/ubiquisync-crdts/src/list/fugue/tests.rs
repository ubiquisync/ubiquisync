use rand::{
    Rng, RngExt, SeedableRng,
    distr::{Alphanumeric, SampleString},
    rngs::ChaCha20Rng,
};

use crate::walker::View;

use super::*;

#[test]
fn sim_list_tests() {
    for i in 0..10 {
        sim_list(ChaCha20Rng::from_seed([i; 32]))
    }
}

fn sim_list(mut rng: impl Rng) {
    const NUM_PEERS: usize = 5;
    const ROUNDS: usize = 500;
    let mut lists: [List<OpId, char>; NUM_PEERS] = Default::default();
    let mut oplog: [Vec<Op<OpId, char>>; NUM_PEERS] = Default::default();
    let mut frontiers: [[usize; NUM_PEERS]; NUM_PEERS] = Default::default();
    for _ in 0..ROUNDS {
        let peer = rng.random_range(0..NUM_PEERS);
        let list = &mut lists[peer];
        let size = list.size(View::Effect);
        let mut string: String = list.iter().collect();

        let ops = if rng.random::<bool>() || size == 0 {
            // insert
            let offset = if size == 0 {
                0
            } else {
                rng.random_range(0..=size)
            };
            let content_size = rng.random_range(1..=16);
            let content = Alphanumeric.sample_string(&mut rng, content_size);
            let insert = list.create_insert(View::Effect, offset, content.chars().collect());
            let op_idx = oplog[peer].len();
            let id = ElementId {
                op_id: (peer, op_idx),
                index: 0,
            };

            // perform the same op against a str to compare
            string.insert_str(offset, &content);

            vec![Op::Insert { id, insert }]
        } else {
            // delete
            let offset = rng.random_range(0..size);
            let count = if offset == size - 1 {
                1
            } else {
                rng.random_range(1..=size - offset)
            };

            // perform the same op against a str to compare
            for _ in 0..count {
                string.remove(offset);
            }

            list.create_delete(View::Effect, offset, count)
        };

        for op in ops {
            list.apply_op(op.clone()).unwrap();
            oplog[peer].push(op);
        }
        frontiers[peer][peer] = oplog[peer].len();

        // compare manipulating a string to the current state of the list
        assert_eq!(string, list.iter().collect::<String>());

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

type OpId = (usize, usize);

// fn sim_eg_walker(mut rng: impl Rng) {
//     const NUM_PEERS: usize = 5;
//     const ROUNDS: usize = 500;

//     enum EgWalkerOp<T> {
//         Observe { frontiers: [usize; NUM_PEERS] },
//         Insert { offset: usize, content: Vec<T> },
//         Delete { offset: usize, count: usize },
//     }

//     let mut lists: [List<OpId, char>; NUM_PEERS] = Default::default();
//     let mut oplog: [Vec<EgWalkerOp<char>>; NUM_PEERS] = Default::default();
//     let mut frontiers: [[usize; NUM_PEERS]; NUM_PEERS] = Default::default();
//     let mut peer_observed: [[Vec<(usize, usize)>; NUM_PEERS]; NUM_PEERS] = Default::default();
//     let mut prepare_state: [Vec<(usize, usize)>; NUM_PEERS] = Default::default();
//     for _ in 0..ROUNDS {
//         let peer = rng.random_range(0..NUM_PEERS);
//         let list = &mut lists[peer];
//         let size = list.size(View::Effect);
//         let mut string: String = list.iter().collect();

//         let (walker_op, list_ops) = if rng.random::<bool>() || size == 0 {
//             // insert
//             let offset = if size == 0 {
//                 0
//             } else {
//                 rng.random_range(0..=size)
//             };
//             let content_size = rng.random_range(1..=16);
//             let content = Alphanumeric.sample_string(&mut rng, content_size);

//             // perform the same op against a str to compare
//             string.insert_str(offset, &content);

//             let content: Vec<char> = content.chars().collect();
//             let insert = list.create_insert(View::Effect, offset, content.clone());
//             let op_idx = oplog[peer].len();
//             let id = ElementId {
//                 op_id: (peer, op_idx),
//                 index: 0,
//             };

//             (
//                 EgWalkerOp::Insert { offset, content },
//                 vec![Op::Insert { id, insert }],
//             )
//         } else {
//             // delete
//             let offset = rng.random_range(0..size);
//             let count = if offset == size - 1 {
//                 1
//             } else {
//                 rng.random_range(1..=size - offset)
//             };

//             // perform the same op against a str to compare
//             for _ in 0..count {
//                 string.remove(offset);
//             }

//             (
//                 EgWalkerOp::Delete { offset, count },
//                 list.create_deletes(View::Effect, offset, count),
//             )
//         };

//         for op in list_ops {
//             list.apply_op(op.clone()).unwrap();
//         }
//         oplog[peer].push(walker_op);
//         frontiers[peer][peer] = oplog[peer].len();

//         // compare manipulating a string to the current state of the list
//         assert_eq!(string, list.iter().collect::<String>());

//         // advance each peer's frontier by a random amount
//         // to simulate inconsistent intermediate states
//         for i in 0..NUM_PEERS {
//             for j in 0..NUM_PEERS {
//                 if i == j {
//                     // skip trying to echo self
//                     continue;
//                 }

//                 let cur_read_size = frontiers[i][j];
//                 let available_size = oplog[j].len();
//                 if cur_read_size == available_size {
//                     continue;
//                 }
//                 let new_read_size = rng.random_range(cur_read_size..=available_size);
//                 let list = &mut lists[i];
//                 // update prepare state
//                 let this_peers_frontiers = peer_observed[i][j];
//                 for i in 0..NUM_PEERS {}

//                 for op in oplog[j][cur_read_size..new_read_size].iter() {
//                     // TODO
//                 }
//                 frontiers[i][j] = new_read_size;
//             }
//             oplog[i].push(EgWalkerOp::Observe {
//                 frontiers: frontiers[i],
//             });
//         }
//     }

//     // at the advance all peer frontiers to the end
//     for i in 0..NUM_PEERS {
//         for j in 0..NUM_PEERS {
//             if i == j {
//                 // skip trying to echo self
//                 continue;
//             }

//             let cur_read_size = frontiers[i][j];
//             for op in oplog[j][cur_read_size..].iter() {
//                 lists[i].apply_op(op.clone()).unwrap();
//             }
//         }
//     }

//     // confirm all peers materialized identical state
//     let strings = lists.map(|l| l.iter().collect::<String>());
//     for i in 1..NUM_PEERS {
//         assert_eq!(strings[0], strings[i]);
//     }
// }
