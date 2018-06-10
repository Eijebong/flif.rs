#![allow(unused)]

use std::io::Read;

use colors::{Channel, ColorSpace, ColorValue};
use components::transformations::ColorRange;
use components::transformations::Transform;
use error::*;
use numbers::chances::{ChanceTable, UpdateTable};
use numbers::near_zero::NearZeroCoder;
use numbers::rac::{Rac, RacRead};
use DecodingImage;
use FlifInfo;
use Limits;

mod pvec;
pub(crate) use self::pvec::{core_pvec, edge_pvec};

pub struct ManiacTree<'a> {
    nodes: Vec<ManiacNode<'a>>,
}

impl<'a> ManiacTree<'a> {
    pub fn new<R: Read>(
        rac: &mut Rac<R>,
        channel: Channel,
        info: &FlifInfo,
        update_table: &'a UpdateTable,
        limits: &Limits,
    ) -> Result<ManiacTree<'a>> {
        let context_a = ChanceTable::new(update_table);
        let context_b = ChanceTable::new(update_table);
        let context_c = ChanceTable::new(update_table);

        let prange = Self::build_prange_vec(channel, info);
        let nodes = Self::create_nodes(
            rac,
            &mut [context_a, context_b, context_c],
            update_table,
            prange,
            limits,
        )?;

        Ok(ManiacTree { nodes })
    }

    pub fn size(&self) -> usize {
        use self::ManiacNode::*;

        let mut size = 0;
        let mut stack = vec![0];
        loop {
            let index = match stack.pop() {
                Some(index) => index,
                None => break size,
            };

            size += 1;
            match self.nodes[index] {
                Property { .. } | InactiveProperty { .. } | Inner { .. } => {
                    stack.push(2 * index + 2);
                    stack.push(2 * index + 1);
                }
                _ => {
                    continue;
                }
            };
        }
    }

    pub fn depth(&self) -> usize {
        use self::ManiacNode::*;

        let mut largest_depth = 0;

        let mut stack = vec![(0, 1)];
        loop {
            let (index, depth) = match stack.pop() {
                Some(index) => index,
                None => break largest_depth,
            };

            largest_depth = ::std::cmp::max(largest_depth, depth);

            match self.nodes[index] {
                Property { .. } | InactiveProperty { .. } | Inner { .. } => {
                    stack.push((2 * index + 2, depth + 1));
                    stack.push((2 * index + 1, depth + 1));
                }
                _ => {
                    continue;
                }
            };
        }
    }

    pub fn process<R: Read>(
        &mut self,
        rac: &mut Rac<R>,
        pvec: &[ColorValue],
        guess: ColorValue,
        min: ColorValue,
        max: ColorValue,
    ) -> Result<ColorValue> {
        if min == max {
            return Ok(min);
        }

        let val = self.apply(rac, pvec, min - guess, max - guess)?;
        Ok(val + guess)
    }

    fn create_nodes<R: Read>(
        rac: &mut Rac<R>,
        context: &mut [ChanceTable; 3],
        update_table: &'a UpdateTable,
        prange: Vec<ColorRange>,
        limits: &Limits,
    ) -> Result<Vec<ManiacNode<'a>>> {
        use self::ManiacNode::*;

        let mut result_vec = vec![];
        let mut node_count = 0;
        let mut process_stack = vec![(0, prange)];
        loop {
            let (index, prange) = match process_stack.pop() {
                Some(process) => process,
                _ => break,
            };

            if node_count > limits.maniac_nodes {
                Err(Error::LimitViolation(format!(
                    "number of maniac nodes exceeds limit"
                )))?;
            }

            node_count += 1;
            let node = if index == 0 {
                Self::create_node(rac, context, update_table, &prange)?
            } else {
                Self::create_inner_node(rac, context, &prange)?
            };

            if index >= result_vec.len() {
                result_vec.resize(index + 1, ManiacNode::InactiveLeaf);
            }

            let (property, test_value) = match node {
                Property { id, value, .. }
                | InactiveProperty { id, value, .. }
                | Inner { id, value } => (id, value),
                _ => {
                    result_vec[index] = node;
                    continue;
                }
            };

            let mut left_prange = prange.clone();
            left_prange[property as usize].min = test_value + 1;

            let mut right_prange = prange;
            right_prange[property as usize].max = test_value;

            process_stack.push((2 * index + 2, right_prange));
            process_stack.push((2 * index + 1, left_prange));
            result_vec[index] = node;
        }

        Ok(result_vec)
    }

    fn create_node<R: Read>(
        rac: &mut Rac<R>,
        context: &mut [ChanceTable; 3],
        update_table: &'a UpdateTable,
        prange: &[ColorRange],
    ) -> Result<ManiacNode<'a>> {
        let chance_table = ChanceTable::new(update_table);
        let mut property = rac.read_near_zero(0, prange.len() as isize, &mut context[0])?;

        if property == 0 {
            return Ok(ManiacNode::Leaf(chance_table));
        }
        property -= 1;

        let counter = rac.read_near_zero(1 as i32, 512 as i32, &mut context[1])?;
        let test_value = rac.read_near_zero(
            prange[property as usize].min,
            prange[property as usize].max - 1,
            &mut context[2],
        )?;

        Ok(ManiacNode::Property {
            id: property,
            table: chance_table,
            value: test_value,
            counter: counter as u32,
        })
    }

    fn create_inner_node<R: Read>(
        rac: &mut Rac<R>,
        context: &mut [ChanceTable; 3],
        prange: &[ColorRange],
    ) -> Result<ManiacNode<'a>> {
        let mut property = rac.read_near_zero(0, prange.len() as isize, &mut context[0])?;

        if property == 0 {
            return Ok(ManiacNode::InactiveLeaf);
        }
        property -= 1;

        let counter = rac.read_near_zero(1 as i32, 512 as i32, &mut context[1])?;
        let test_value = rac.read_near_zero(
            prange[property as usize].min,
            prange[property as usize].max - 1,
            &mut context[2],
        )?;

        Ok(ManiacNode::InactiveProperty {
            id: property,
            value: test_value,
            counter: counter as u32,
        })
    }

    pub fn apply<R: Read>(
        &mut self,
        rac: &mut Rac<R>,
        pvec: &[ColorValue],
        min: ColorValue,
        max: ColorValue,
    ) -> Result<ColorValue> {
        use self::ManiacNode::*;
        let mut node_index = 0;
        loop {
            let (lnodes, rnodes) = &mut self.nodes.split_at_mut(node_index + 1);
            let node = &mut lnodes[node_index];
            match node {
                Inner { id, value } => {
                    if pvec[*id as usize] > *value {
                        node_index = 2 * node_index + 1;
                    } else {
                        node_index = 2 * node_index + 2;
                    }
                }
                Leaf(table) => {
                    return rac.read_near_zero(min, max, table);
                }
                node => {
                    let (val, new_node) = match node {
                        Property {
                            id,
                            value,
                            counter: 0,
                            table
                        } => {
                            let mut left_table = table.clone();
                            let mut right_table = table.clone();

                            let val = if pvec[*id as usize] > *value {
                                rac.read_near_zero(min, max, &mut left_table)?
                            } else {
                                rac.read_near_zero(min, max, &mut right_table)?
                            };

                            rnodes[node_index].activate(left_table);
                            rnodes[node_index + 1].activate(right_table);
                            (
                                val,
                                Inner {
                                    id: *id,
                                    value: *value
                                },
                            )
                        }
                        Property { counter, table, .. } => {
                            *counter -= 1;
                            return rac.read_near_zero(min, max, table);
                        }
                        _ => panic!(
                            "improperly constructed tree, \
                             inactive node reached during traversal"
                        ),
                    };
                    *node = new_node;
                    return Ok(val);
                }
            }
        }
    }

    fn build_prange_vec(channel: Channel, info: &FlifInfo) -> Vec<ColorRange> {
        let mut prange = Vec::new();

        let transform = &info.transform;

        if channel == Channel::Green || channel == Channel::Blue {
            prange.push(transform.range(Channel::Red));
        }

        if channel == Channel::Blue {
            prange.push(transform.range(Channel::Green));
        }

        if channel != Channel::Alpha && info.header.channels == ColorSpace::RGBA {
            prange.push(transform.range(Channel::Alpha));
        }

        prange.push(transform.range(channel));
        prange.push(ColorRange { min: 0, max: 2 });

        let maxdiff = ColorRange {
            min: transform.range(channel).min - transform.range(channel).max,
            max: transform.range(channel).max - transform.range(channel).min,
        };
        prange.push(maxdiff);
        prange.push(maxdiff);
        prange.push(maxdiff);
        prange.push(maxdiff);
        prange.push(maxdiff);

        prange
    }
}

#[derive(Clone)]
enum ManiacNode<'a> {
    /// Denotes a property node, property nodes are nodes that currently act as leaf nodes but will become inner nodes when their counter reaches zero
    Property {
        id: isize,
        value: i16,
        table: ChanceTable<'a>,
        counter: u32,
    },
    InactiveProperty {
        id: isize,
        value: i16,
        counter: u32,
    },
    /// Inner nodes are property nodes whose counters have reached zero. They no longer have a context associated with them.
    Inner {
        id: isize,
        value: i16,
    },
    /// Leaf nodes are nodes that can never become inner nodes
    Leaf(ChanceTable<'a>),
    InactiveLeaf,
}

impl<'a> ManiacNode<'a> {
    // return type is temporary, will be some reasonable pixel value
    pub fn activate(&mut self, table: ChanceTable<'a>) {
        use self::ManiacNode::*;
        *self = match self {
            InactiveLeaf => Leaf(table),
            InactiveProperty { id, value, counter } => Property {
                id: *id,
                value: *value,
                counter: *counter,
                table: table,
            },
            _ => return,
        }
    }
}
